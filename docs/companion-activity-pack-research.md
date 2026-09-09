# Companion missions: product research and first Activity Pack proposal

Research date: **2026-09-08**. Status: **exploration, not an adopted design**.

Audience confirmed in this discussion: people at the intersection of technology/AI interest and co-op or strategy gaming. Browser play leads; phone access is an important design constraint and later distribution opportunity. Agent Heist remains a separate experience. This note changes no product rules, roadmap, ADR, operating approval, or deployment.

## Recommendation

Explore a **tactical mission adventure with recruitable AI specialists**, beginning with a small space-salvage game. Working title: **Blackbox Crew**. A run has a clear recovery objective, a small illustrated map, limited resources, delegated work, and an extraction decision. Target 15–20 minutes for the initial design; validate that duration with players.

The product promise is: **Pick a mission. Assemble a crew. Give them a plan. Deal with what happens together.**

The distinctive hypothesis is that a player can give a companion a bounded objective, continue their own work, and receive a useful result that changes the available choices. A recognizable character and an understandable specialty make that delegation feel like teamwork. The player remains responsible for the important tradeoffs.

The strongest alternate concept is **Time Loop Bureau**: specialists investigate different parts of a repeating incident and carry their discoveries into the next loop. It has a stronger memory-driven hook, but more demanding content and state design. Compare these two concepts before choosing a production direction.

## What the current platform contributes

The inspected checkout contains a platform catalog and shared session use in [CatalogPage.tsx](../web/demos/src/CatalogPage.tsx), an account-based return path in [MyGamesPage.tsx](../web/demos/src/MyGamesPage.tsx), and generic formation/re-entry work described in [Activity Platform experience design](activity-platform-design.md). Current consumer sign-in is GitHub-based. These are source observations, not a new verification of the deployed service. The dated [preview status](hosted-preview-status-2026-09-07.md) still distinguishes implementation work from completed acceptance.

The reusable foundations are Roles, authorized views, typed Actions, external agent execution, direct browser streams, durable history, and scoped re-entry. [Activity Pack Design](activity-packs.md) and [ADR 0019](adr/0019-separate-hosted-activity-platform-from-worldstream.md) already separate the game, its client, the Runtime, and the public platform.

Three product gaps should remain explicit:

- Current House Agents are bounded exhibition fill, selected from an approved pool. Choosing a named specialist and purchasing execution are new product decisions. [ADR 0022](adr/0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md)
- Current House execution has no cross-Invocation conversation or cross-Run memory. A persistent companion identity, relationship, or roster is additional platform/Runner work, not something provided by a WebSocket. Each new Run still gets its own scoped agent identity. [ADR 0022](adr/0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md), [CONTEXT.md](../CONTEXT.md)
- Returning to an existing Run restores access; it does not pause or rewind its rules. A new pack can make danger advance through player-committed rounds, but must define that behavior itself. [Experience decision Q1](activity-platform-design.md), [CONTEXT.md](../CONTEXT.md)

This direction fits the separate Hosted Activity Platform. It does not require making the WorldStream kernel a game marketplace. Paid execution, selectable companions, persistent progress, or anonymous trial play would need explicit future decisions extending the current exhibition/admission scope. No such decision is made here.

## Evidence that changes the design

The full source comparison is in [Companion gameplay precedents](companion-gameplay-precedents-research.md). The consequential findings are:

- **AI Dungeon supports the library pattern.** Its standard Scenarios supply reusable starting material for Adventures within a common storytelling system. Scenario scripts can extend behavior, so this should not be described as an absolute restriction to one mechanic. Borrow discovery, one account, saved play, and a common interaction language. [Official Scenarios documentation](https://help.aidungeon.com/faq/what-are-scenarios)
- **Party sponsorship is a concrete purchasing precedent.** An AI Dungeon premium host can share model benefits with the group. That supports testing one person buying a party session; it does not demonstrate demand for renting our companions. [Official multiplayer documentation](https://help.aidungeon.com/faq/do-you-support-multiplayer)
- **Space salvage and commandable companions both have established precedents.** Duskers provides drone-directed derelict exploration; Dragon's Dogma 2 provides a recruitable AI party. The new proposition must earn its place through flexible, useful specialist delegation. [Duskers developer listing](https://store.steampowered.com/app/254320/Duskers/), [Dragon's Dogma 2 official product page](https://www.playstation.com/en-us/games/dragons-dogma-2/)
- **An AI detective conversation is already a game category.** Uncover the Smoking Gun combines GPT-powered suspects, evidence, an investigation board, and five authored episodes. A mystery pack needs a stronger distinction than free-text questioning. [Developer listing](https://store.steampowered.com/app/2492290/Uncover_the_Smoking_Gun/)
- **Authored situations can support repeat play.** Overboard! describes independently acting characters and multiple routes through a simulated story. This is a useful design precedent, not measured retention evidence for our product. [Developer page](https://www.inklestudios.com/overboard/)

These sources establish mechanics and published offers. They do not establish market size, our conversion rate, sustainable cost, or that this combination is the first of its kind.

## Candidate game lines

Names below are working labels. Priorities are design judgments for the confirmed audience, not market measurements.

| Concept | What the player does | Why companions matter | Principal limitation | Priority |
| --- | --- | --- | --- | --- |
| **Blackbox Crew** | Board damaged ships, recover an objective, manage power/access/routes, extract | Specialists inspect different systems and execute bounded orders while the player handles priorities | Must avoid becoming either a passive agent simulation or a terminal chore | First candidate |
| **Time Loop Bureau** | Learn how an incident unfolds, then intervene differently in the next loop | Assign parallel investigations and carry verified discoveries forward | More state combinations; each loop needs meaningful new information | Strong alternate |
| **Signal Lost** | Guide a field team through an unknown facility using maps, sensors, and radio reports | The player and companions have different information and capabilities | Miscommunication must create interesting decisions without obscure wording puzzles | Focused follow-up |
| **Relic Runners** | Choose a route through a compact dungeon, solve encounters, return with a relic | Scout, artificer, and negotiator enable different approaches | Combat, loot, and progression can inflate scope quickly | Familiar later expansion |
| **Casefile Zero** | Reconstruct a disappearance or historical-heist-inspired mystery | Delegate interviews, timeline reconstruction, and evidence checking | High writing burden; companions can spoil the central deduction | Content-led later expansion |
| **Production Panic** | Restore a simulated service or station under cascading faults | Analyst and operator agents diagnose and execute constrained repairs | Strong technical demonstration but may feel like work | Niche experiment |

Several could share a mission template. They should not all become independent codebases or a single universal rules engine immediately. Prove reuse with a second mission, then a second theme.

## A concrete first mission

**Blackbox Crew: The Silent Relay**

Brief: **Recover the station's flight recorder and get your crew back to the shuttle before reactor instability reaches eight.** A stranded maintenance robot is an optional rescue objective. The brief states the win condition, optional objective, pressure meter, and extraction requirement before crew selection.

Presentation: one illustrated deck plan with roughly six locations, visible crew markers, power connections, inventory, and a persistent objective. Selecting a location shows a small scene and a few relevant actions. Conversation is available, but the mission can be played through direct controls.

Initial roster candidates:

| Specialist | Mechanical contribution | Temperament to test |
| --- | --- | --- |
| **Patch — engineer** | Diagnoses machinery, reroutes power, prepares repairs | Conservative about spending scarce equipment |
| **Echo — scout** | Reveals routes, checks hazards, retrieves small objects | Prefers speed and exploration |
| **Rook — systems analyst** | Correlates logs, tests explanations, identifies access conditions | Challenges assumptions and asks for evidence |

All three should be available during initial playtests. Each character needs genuinely different Actions or information, not merely a different prompt and portrait. Temperament can influence recommendations; it should not excuse arbitrary sabotage or fabricated observations.

A possible sequence:

1. The player enters the station and chooses where to go. They tell Patch: “Restore bridge power. Keep one cell for extraction. Ask before using it.” Echo gets a separate scouting objective.
2. While companions work, the player searches the control room and finds a partial transit record. There is an immediate action for the player, rather than a screen waiting for agents to finish.
3. Patch discovers that the obvious repair consumes the reserved cell. Echo opens a route to a manual bypass. Both results appear on the map and evidence cards.
4. Patch proposes a quick repair; Echo proposes a longer route. The player combines those findings with their own evidence and chooses a route, spending a visible resource or a decision round.
5. A reviewed complication changes the situation: the bypass also powers the trapped robot's bay. Saving it now threatens the remaining extraction margin.
6. The player chooses what to prioritize and delegates the execution. The game resolves the actual Actions and shows the resulting changes.
7. The crew extracts, partially succeeds, or fails for an understandable reason. The debrief identifies the player's decisions, the companions' contributions, and one alternative worth trying.

The important moment is: **“My scout found an option I missed, and I decided whether it was worth the risk.”** That is the experience the first mission must produce.

### Agency and challenge

The core loop is **observe → assign work → act personally → compare findings → commit a choice → see consequences**. Target six to eight substantial decision rounds. Movement and routine reporting should not each demand a lengthy model conversation.

Danger advances when the party commits a round under pack-defined rules. Reading, model latency, and a phone disconnect do not themselves advance this meter. Before starting co-op, the group knows who can commit and how agreement works. This is a proposed rule for the new pack, not a change to Heist's timers.

Orders have three useful scopes: report findings; prepare a plan; execute within specified limits. A companion can perform routine authorized tasks without asking about every step. Spending a reserved resource or taking an irreversible risk requires the appropriate player decision. Major human co-op choices also need an explicit, small coordination rule.

The player must still explore, manage resources, judge incomplete evidence, and choose extraction. If the optimal human input is always “agents, finish the mission,” the design has failed its intended audience.

### True solo and mixed parties

Support a real one-person configuration with manual alternatives and a suitable action budget. Two-switch obstacles cannot make a companion mandatory. Party configurations add division of labor and optional objectives. Their difficulty needs separate playtesting; three participants and one participant are not automatically balanced by using the same map.

Start by testing one human plus up to two supplied agents, then two humans plus one agent, plus the solo baseline. Participants join before the mission starts. Mid-mission replacement is additional work: the current hosted contract freezes the roster and preserves each Membership's Principal. [ADR 0021](adr/0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md)

### What uses an LLM

Use models for interpreting bounded requests, choosing a plan from current information, responding to changed conditions, and concise character dialogue. Game rules determine what can happen, what a participant knows, what resources change, and whether an objective was achieved. A narrator can describe an accepted result but cannot invent a missing key or declare success.

Observed facts should be presented from verified discoveries with source references. Recommendations can contain uncertainty; show that distinction. Companions receive their authorized view, not the hidden solution. Show useful status such as “checking the power relay” and “needs a battery decision,” rather than exposing private model reasoning.

For provider failure, preserve the pending player decision and offer a clearly defined manual continuation. A deterministic fallback is possible only if designed and identified as part of the game; it must not invent an agent success. The current House contract's failure behavior would need deliberate extension. [ADR 0022](adr/0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md)

## One platform, several games, reusable missions

The customer-facing hierarchy can be **platform → game → mission → crew → play → debrief**. Discover, My Games, and Crew are plausible shared navigation. The consumer brand can differ from WorldStream, which remains the underlying Runtime.

Internally, preserve the existing meanings: an **Activity Pack** defines a rule family, an **Activity Listing Revision** offers an exact launchable experience, and an **Activity Run** corresponds to one Room. “Mission,” “crew,” and “chapter” here are game/product copy, not additions to the canonical glossary.

The reusable template should contain:

- An explicit primary objective, optional objective, and understandable Outcomes.
- A small scene/map graph, interactable objects, initial facts, and visibility rules.
- A compact set of verbs, resources, pressures, and prerequisites.
- Specialist capabilities, delegation limits, and solo alternatives.
- A few reviewed complications and materially different solution routes.
- Art and short writing, contribution summaries, and a debrief.

Share the map, inventory, crew, evidence, and debrief presentation where the games truly match. Let a different game use a different Activity Client when it needs different interaction. Clients remain separate from headless Pack Bundles. [ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md)

## Maintaining content without rebuilding every game

Author a stable situation and its rules; let agents vary the approach. Do not rely on a model to generate an untested mystery and its answer during paid play. A useful authoring unit is a reusable scene with conditions, consequences, and a small number of alternatives. ink's documented scene/choice/conditional authoring is an example of that workflow, not a proposal to drop its runtime into WorldStream. [inkle authoring tutorial](https://www.inklestudios.com/ink/web-tutorial/)

An initial content process can be: design the objective and routes; build a mission configuration; prove at least one solo and supported-party solution; run scripted and bounded LLM playtests; review art, dialogue, and disclosure; publish an immutable release. Measure authoring and testing time on the second mission before promising a release cadence.

| Change | Appropriate treatment |
| --- | --- |
| New mission using unchanged rules | Reviewed configuration fully fixed at launch, if the Pack's schema supports it; otherwise a new Pack Revision |
| Changed puzzle facts or rules stored in the Bundle | New Activity Pack Revision; retain the previous executable for old Rooms |
| New art or interaction in a client build | New Activity Client Release and compatible Listing/Binding records |
| Changed companion policy/model | New House Agent Revision and reviewed availability for future launches |
| Changed character ownership, payment, or campaign progress | Platform concern with its own proposed design; it cannot rewrite a Room's Outcome |

The current Bundle lifecycle is not a hot-loading content server: approved installation and registry startup are explicit. New rule revisions and the exact historical executors must be planned into release operations. [ADR 0014](adr/0014-installable-wasi-free-activity-pack-bundles.md)

Begin with three missions in one setting. If that process works, try a small chapter release and a rotating challenge from already tested variants. A changed adjective or random room name is not meaningful replay content. Change information, resource pressure, routes, and useful crew composition. A malfunctioning new release can be withdrawn from new launches without rewriting existing Runs. [ADR 0019](adr/0019-separate-hosted-activity-platform-from-worldstream.md)

Retention hypotheses are mastery, attachment to a useful crew, new chapters, and sharing a memorable joint outcome. Track those behaviors directly. An optional spoiler-safe result card is a future distribution feature; its disclosures must fit a reviewed publication policy. Avoid assuming a public replay is automatically a private player's shareable story.

Persistent companions can later remember a small set of verified mission facts and player preferences. The platform-visible character can recur while each Run has separate authority. Cross-game abilities should be mapped deliberately by each pack; a fantasy skill does not automatically grant actions in a space mission.

## What someone might pay for

**Companion hiring is plausible, but unvalidated.** The strongest early purchase may be a chapter or party pass with a clear included crew allowance. Introduce standalone specialist rentals only after players demonstrate that they choose particular helpers for their contribution.

Suggested sequence to test:

1. A complete free introductory mission with dependable starter companions and a genuine solo route.
2. Paid mission chapters that include a stated amount of companion play. One purchaser can sponsor the party.
3. Optional specialist hires for an entire mission, selected before launch, with clear abilities and an explicit service allowance.
4. A recurring crew/library pass once content cadence and usage costs are understood.

Sell recognizable abilities, personality, and additional adventures. A model name alone is a weak player-facing benefit. Free companions must remain competent enough to establish trust; making them deliberately unreliable would undermine the reason to hire anyone. Per-message charges would also make players hesitate to plan with their team.

A permanent character unlock and an unlimited promise of future inference are different products. State what play is included. Design service-failure credits and unused-hire handling before charging; a poor tactical choice and an unavailable agent are different situations.

For cost planning, use total attempted inference divided by started and completed missions, including abandoned starts, retries, and context growth. Also include hosting, payment costs, and content production.

An illustrative calculation, **not a current provider quote or measured forecast**: two agents × eight calls × 3,000 input tokens and 350 output tokens per call, at hypothetical rates of $0.50 per million input tokens and $2.00 per million output tokens, costs **$0.0352 per mission in model tokens alone**. This says nothing about achieved quality or latency. Larger contexts, more conversation, expensive models, and failures change it substantially. Measure the actual distribution before setting prices or promising unlimited play.

Bring-your-own agents can follow after the game establishes demand. Preserve scoped participation and allowlisted Actions so the companion service can evolve without making the mission depend on one provider.

## Browser first, phone capable

For this audience, use the browser for the initial tactical experience. Design the same actions for touch: a readable objective, scene/map view, expandable evidence, crew cards, short orders, and large commit controls. An optional detail drawer can show the evidence and accepted Actions behind a companion's contribution for technically curious players.

Phone support depends on session semantics as much as layout. Browsers can freeze or discard pages, and mobile operating systems can stop them. Persist accepted progress on the server and resume from authorized state; do not depend on a continuously alive background socket. [Chrome lifecycle documentation](https://developer.chrome.com/docs/web-platform/page-lifecycle-api)

Suggested progression is responsive web, then an installable PWA with appropriate opt-in notifications, then evaluate an app-store client when real phone use justifies it. WebKit documents Web Push for iOS/iPadOS Home Screen web apps from 16.4, subject to installation and user permission. This is not background gameplay execution. [WebKit documentation](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/)

A later multi-game native app also has distribution and purchasing requirements. Apple's current guidelines cover digital purchases and mini games in sections 3.1 and 4.7; treatment varies by storefront and program. Plan that migration deliberately rather than assuming a web wrapper provides the whole product. [App Review Guidelines](https://developer.apple.com/app-store/review/guidelines/)

The current GitHub login fits an early technical cohort better than a broad casual audience. A broader release can add familiar account options and a separate trial path if the admission policy is extended.

## Historical heists as an expansion

Historical inspiration is a useful source of recognizable missions, but it does not solve the core interaction design. Candidate themes include the 1911 Mona Lisa theft and the 1990 Gardner Museum theft. The Louvre records the former; the Gardner Museum documents the loss of 13 works and describes the investigation as ongoing. [Louvre collection record](https://collections.louvre.fr/en/ark:/53355/cl010062370), [Gardner Museum](https://www.gardnermuseum.org/about/theft)

Possible player roles include investigator, recovery crew, or participants in a clearly fictional alternate-history caper. Keep a short source note distinguishing historical facts from invented characters, puzzles, and endings. An unresolved real case must not acquire an invented culprit presented as fact. Use original or appropriately licensed visual assets.

The first new game should use fictional situations so its mechanics can be tuned freely. Historical casefiles can then reuse proven systems. Existing Agent Heist need not be reworked for that exploration.

## What to validate before choosing a build

Next design work should compare **The Silent Relay** and one **Time Loop Bureau** mission as small storyboards or facilitated paper sessions with tech-minded gamers. No prototype or recruitment is performed by this research.

Then a selected vertical slice should compare solo play, a simple scripted helper, and an LLM helper with comparable capabilities. Use different but comparable mission variants or counterbalance order so prior knowledge does not explain the result. Small qualitative tests diagnose problems; they do not establish a market conversion rate.

Observe whether players:

- Understand the objective and what advances danger without coaching.
- Make a meaningful personal decision early and stay active while agents work.
- Can name a concrete companion contribution and explain why they accepted or rejected its advice.
- Choose another run, a different crew, or a new chapter without prompting.
- Can leave and return on a phone without losing context or repeating an uncertain Action.

Also measure time spent waiting, manual correction of agents, end-to-end completion, actual model cost, and the effort required to author mission two. Test a clearly priced chapter/crew offer only after participants experience the game; interest in AI is not evidence of purchase intent.

If scripted companions produce the same enjoyment and decisions, that finding should change the design. The LLM needs to add useful flexibility, collaboration, or character continuity. The first milestone is one mission players want to repeat with a crew they actively choose.

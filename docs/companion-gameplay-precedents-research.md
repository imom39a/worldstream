# Companion gameplay precedents — research / proposal

Researched **2026-09-08**. This is decision support for a possible new Activity Pack, not an adopted design or a change to Agent Heist. It covers external gameplay precedents; it does not assess repository implementation readiness.

The evidence supports exploring **clear missions with recruitable specialists and player-controlled decisions**. It does not establish that people will pay to rent LLM companions, that the proposed combination is novel, or that it will retain players. Those remain product hypotheses.

## What AI Dungeon actually demonstrates

**Catalog and core loop — documented facts.** AI Dungeon's Scenarios are reusable starting templates containing prompts, settings, and contextual material; launching one creates an Adventure. Creators can publish templates for other players. Its standard play loop accepts Do, Say, or Story inputs, generates narrative responses, and permits undo, retry, and editing. Discovery is therefore evidence for many selectable experiences around a shared storytelling system. It is not, by itself, evidence for many independently designed game systems. [Scenario documentation](https://help.aidungeon.com/faq/what-are-scenarios), [core play loop](https://help.aidungeon.com/faq/the-basics).

**Account and mobile — documented facts.** Adventures are stored in the cloud and accessed through the player's account across devices. AI Dungeon offers Android and iOS apps and supports play in mobile browsers. [Account continuity](https://help.aidungeon.com/faq/why-do-i-need-an-account-to-play), [mobile availability](https://help.aidungeon.com/faq/can-i-play-ai-dungeon-on-my-phone).

**Human multiplayer — documented facts.** A host creates an Adventure and shares an invitation code; participants contribute actions to that Adventure. The host's model settings apply to the group, and a premium host can share those benefits with other players. [Multiplayer documentation](https://help.aidungeon.com/faq/do-you-support-multiplayer).

**Business model — documented facts.** The published membership table includes a free plan and paid subscription tiers. Paid benefits include additional models, context, memories, and credits for selected capabilities. These are published offers at retrieval, not measured conversion or revenue. [Memberships and benefits](https://help.aidungeon.com/memberships-benefits).

**Implication — inference.** Borrow the shared discovery, account, saved-play, and party-host purchasing patterns. Separately design a mission loop with explicit objectives and consequences. AI Dungeon's listed benefits do not test a companion-specific rental proposition. Its public play page returned a connection shell to the research browser; the claims above come from official documentation, not a hands-on account or gameplay audit.

## Four useful gameplay precedents

| Precedent | Documented mechanic | Transferable hypothesis and evidence limit |
| --- | --- | --- |
| **Dragon's Dogma 2 — Pawns** | The player leads up to three AI companions. A main Pawn is customizable; two others can be hired from game options or other players online. Pawns have vocations, respond to commands, and support combat and exploration. The product describes this as a single-player adventure. [Official PlayStation product page](https://www.playstation.com/en-us/games/dragons-dogma-2/) | **Hypothesis:** recruitment becomes meaningful when characters have understandable jobs and the player can direct their work. **Limit:** the documented Pawn system is not evidence for LLM companions, live human co-op, or willingness to pay for our proposed rental. “Hire” in a game is not equivalent to a real-money transaction. |
| **Keep Talking and Nobody Explodes** | The Defuser sees the device; Experts have its manual but cannot see the device. The group communicates to solve modules before the timer ends. Missions introduce more modules; Free Play configures challenges. Only one game copy is needed, and mobile versions exist. [Developer website](https://keeptalkinggame.com/) | **Hypothesis:** different information and responsibilities create a concrete reason to communicate with teammates. **Limit:** the official design requires a team. Replacing an Expert with an AI or supporting solo play changes the challenge; neither experience is validated here. Its live countdown also does not establish suitability for interrupted phone sessions. |
| **Lifeline** | An authored branching story asks the player to advise stranded Taylor. New messages arrive over time through notifications; players can respond or catch up later. Alternate paths become accessible after completing a path. The developer's listing says no internet connection, in-app purchases, or ads are required. [Developer Google Play listing](https://play.google.com/store/apps/details?id=com.threeminutegames.lifeline.google&hl=en_US) | **Hypothesis:** short decisions and a recognizable companion can make a small-screen adventure feel personal. **Limit:** short interactions do not mean the whole adventure is short. This is an authored story, not evidence for autonomous LLM teammates, rental revenue, or notification-driven retention for our audience. |
| **Uncover the Smoking Gun** | A released single-player game combines freely typed conversations with GPT-powered robot suspects, scene evidence, an investigation board, and five episodes. Players use evidence and questions to expose the case. [Developer Steam listing](https://store.steampowered.com/app/2492290/Uncover_the_Smoking_Gun/) | **Hypothesis:** free-form language is easier to place inside a game when it helps resolve a concrete evidence problem. **Limit:** AI detective dialogue already has a direct precedent. This does not demonstrate companion parties, phone usability, low content-production effort, or a rental market. Its existence also does not establish reliable dialogue behavior in our implementation. |

## Gameplay criteria to test, not adopt yet

1. **The mission fits in one sentence.** The player should know what they are trying to accomplish, what can go wrong, and how progress is shown before recruiting anyone.
2. **A companion performs useful work.** Examples to test include checking a route, comparing evidence, negotiating a constrained exchange, or preparing an option. A companion should produce an observable contribution beyond commentary.
3. **Recruitment changes approach.** Different specialists should expose different routes or tradeoffs. Selecting a roster is a potentially interesting decision; buying a stronger model is not automatically an interesting decision.
4. **The player owns consequential choices.** Test companions that investigate, propose, and execute bounded assignments while the player chooses the plan and commits major decisions. Measure whether the player feels helped or replaced.
5. **A phone session can end cleanly.** Prefer meaningful decision points with a visible state recap. Test brief completed missions separately from longer asynchronous stories; the precedents support both patterns, not a reason to mix both into the first release.

These are design inferences from the documented mechanics above. They should be tested against a solo baseline and a simple scripted-companion baseline; an LLM needs to improve the experience enough to justify its complexity.

## Remaining uncertainty

- **Companion value:** whether players want delegation, advice, character interaction, or another human most—and whether those preferences vary by mission.
- **Agency:** whether a helper creates better decisions or completes the interesting part for the player.
- **Replay:** whether new rosters and situations produce meaningfully different choices after the solution is known.
- **Payment unit:** whether people prefer new missions, recurring access, a party pass, permanent characters, or per-mission hires. None of the reviewed sources measures our proposed offer.
- **Audience and session shape:** target players' tolerance for reading, typing, waiting, and scheduled group play on phones.
- **Novelty and market fit:** the primary documentation/product sources reviewed here provide precedent screening, not an exhaustive competitor search or evidence of retention, acquisition, revenue, or sustainable operating cost.

## Follow-up: space salvage for tech-interested co-op and strategy players

Added **2026-09-08** after the audience was clarified as the intersection of people interested in technology/AI and co-op/strategy gamers. The following three additional sources assess a browser-first space-salvage concept; phone usability remains a design constraint rather than the initial audience definition.

**Duskers — documented facts.** The player commands drones and ship systems through a command-line interface while exploring derelict spacecraft. Perception comes through drone views, microphones, and imperfect motion sensors. Salvage improves equipment, while equipment failures, scarce weapons, fuel, and parts constrain the player's choices. The developer lists the game as single-player. [Misfits Attic's Steam listing](https://store.steampowered.com/app/254320/Duskers/).

**Inference and limit.** A remote-operations setting makes partial information, tool use, and delegation natural parts of a mission. A browser interface could present a ship schematic, instrument readings, and assigned specialist work. However, directing drones through explicit commands does not demonstrate an autonomous LLM party. Space salvage plus a terminal is an existing design space; our candidate difference would need to be the quality of delegation, coordination, and resulting decisions.

**Deep Rock Galactic — documented facts.** Its developer/publisher listing describes one-to-four-player missions involving exploration, resource extraction, and survival. Four classes contribute different tools: Scout provides mobility and light; Gunner provides firepower and shielding; Engineer provides defensive tools; Driller reshapes routes through terrain. [Official Steam listing](https://store.steampowered.com/app/548430/Deep_Rock_Galactic/).

**Bosco — historical developer rationale.** In its Early Access balancing retrospective, Ghost Ship Games describes Bosco as a flying solo helper for mining, combat, and revival. The director explicitly treats Bosco as less capable than another human player and discusses scaling enemies by player count and the difficulty of making every mission solo-friendly. This is a historical account of the design tradeoff, not a complete inventory of Bosco's current capabilities. [Developer update archive, “The Challenges of Balancing an Early Access Game”](https://www.deeprockgalactic.com/updatesold).

**Implication — proposal to test.** Build the candidate around a clear recovery objective, limited knowledge, complementary tools, and an extraction decision. Specialists should change routes, risks, or available resources through visible actions. Compare solo play with a bounded helper against human co-op and LLM teammates; substituting one participant type for another will need its own mission and difficulty evaluation. These precedents support concrete companion utility and team composition as design material. They establish neither browser performance, LLM reliability, willingness to rent agents, nor retention for the proposed audience.

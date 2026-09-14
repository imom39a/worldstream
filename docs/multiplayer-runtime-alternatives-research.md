# Game client and multiplayer runtime direction

Research date: September 11, 2026. This is a recommendation, not an accepted
architecture change or a performance qualification. No runtime implementation,
deployment, provider calls, or benchmark was performed for this review.

## Scope and bottom line

This compares the three closest open-source alternatives to WorldStream’s Rust authoritative Room runtime: Colyseus, Nakama’s open-source server, and boardgame.io. It covers room/match authority, WebSocket realtime transport, private per-member views, persistence/reconnect/replay, and agent/AI integration. “Supported” means a documented primitive; a feature achievable by writing application code is called custom. This is not a popularity or performance ranking.

The overlap is substantial: all three can host server-controlled game logic and connect clients in realtime. WorldStream’s proposed combination—immutable Activity Pack revisions, typed Action Offers, durable canonical transition history, deterministic replay, and explicit external Runner boundaries—should therefore be treated as a product/semantic architecture choice, not assumed novel merely because the runtime is custom. The alternatives do not present that exact contract as one first-class abstraction in the reviewed sources.

## Colyseus

Colyseus is an open-source Node.js framework whose core unit is a `Room`: each instance owns state, logic, and connected clients; clients send messages and the server mutates state. The default transport is WebSocket, and the framework automatically sends schema-based property-level delta patches ([overview](https://docs.colyseus.io/), [Rooms](https://docs.colyseus.io/room), [State Synchronization](https://docs.colyseus.io/state)). This is the closest transport/room analogue to WorldStream.

Private projections are first-class but schema-oriented, not a WorldStream-style projection contract: `StateView` can tag fields and add/remove schema instances per client, including team-owned or private fields. The docs explicitly say the entire state is visible by default and caution that StateView is not optimized for large datasets ([StateView](https://docs.colyseus.io/state/view)). Typed messages and input validation are available, but “typed Action Offer” semantics (legality, expiry, provenance, durable offer identity) would be custom Room code.

Reconnect is first-class: the server can hold a seat with `allowReconnection()`, and clients can use automatic retry or a stored reconnection token ([Reconnection](https://docs.colyseus.io/room/reconnection)). Timers are first-class through the Room clock ([Rooms, clock](https://docs.colyseus.io/room#clock)). The reviewed Room/state docs describe live mutable state, patches, and reconnection, but do not establish an immutable durable transition log, deterministic replay protocol, or durable Room lineage; persistence would be application/database code (the project documents database services and “bring your own database,” but that is not the same as canonical replay).

Agent integration is custom Node/TypeScript application logic: Room handlers can process arbitrary messages and async request/response handlers. There is no documented external-agent Runner boundary. Colyseus and its repository are MIT licensed ([repository](https://github.com/colyseus/colyseus), [license](https://github.com/colyseus/colyseus/blob/master/LICENSE)).

## Nakama OSS core

Nakama is a broader game backend (Apache-2.0) with WebSockets, authentication, storage, matchmaking, and authoritative or relayed multiplayer ([repository](https://github.com/heroiclabs/nakama), [license](https://github.com/heroiclabs/nakama/blob/master/LICENSE)). In authoritative matches, custom server runtime code validates inputs, runs a fixed-tick match loop, and broadcasts data; state is an isolated in-memory region and broadcasts are explicit ([authoritative multiplayer](https://heroiclabs.com/docs/nakama/concepts/multiplayer/authoritative/)). This covers authoritative rooms/matches and timers/ticks, but the game’s action schema, legality, and visibility are developer-defined.

Nakama supports custom per-recipient messaging (the match handler chooses what to broadcast), but the cited authoritative docs do not describe a first-class private projection/state-view model comparable to Colyseus StateView. Treat per-membership privacy as custom match code. Persistence is possible through the storage engine: the multiplayer docs describe storing match state and, for passive turn-based matches, writing state to the database when participants go offline ([multiplayer engine](https://heroiclabs.com/docs/nakama/concepts/multiplayer/)). That is useful durable state, but the sources do not promise an immutable append-only transition history, deterministic replay verification, or automatic reconnect to a durable match lineage. Match lifecycle and reconnect semantics therefore need application design.

Agent/AI-style integration is unusually practical as custom server code: Nakama supports Lua, TypeScript/JavaScript, and native Go runtime extensions, plus RPCs and database access ([repository features](https://github.com/heroiclabs/nakama#features)). The docs do not define an external Runner protocol, so process isolation, capabilities, retries, and typed offers remain custom. Apache-2.0 is permissive, but the server’s broader operational footprint (including a Postgres-wire-compatible database) is heavier than a small standalone Room server ([repository setup](https://github.com/heroiclabs/nakama#docker)).

## boardgame.io

boardgame.io is an MIT-licensed JavaScript/TypeScript engine explicitly aimed at turn-based games. Its README says game rules are move functions and that state, realtime multiplayer, storage, phases, lobby, AI bots, and logs/time travel are provided by the engine ([README](https://github.com/boardgameio/boardgame.io), [license](https://github.com/boardgameio/boardgame.io/blob/main/LICENSE)). It is the strongest fit for a simple turn-based JS game and the weakest fit for continuous realtime simulation.

It provides a server/client multiplayer model and storage adapters, and its game model has phases/turn order. Logs can support viewing earlier states (“time travel”), but that is not evidence of WorldStream’s immutable canonical transition lineage or deterministic cross-version replay contract. Player-specific visibility is represented by the game framework’s filtered state/player-view mechanisms, but exact privacy rules and integrations should be verified against the current API before treating them as equivalent to per-Membership Projections. AI is a documented built-in direction (bots), while external agent processes, capability-scoped Runners, and durable action-offer workflows would be custom.

## Recommendation

For a greenfield simple JavaScript multiplayer game, start with an existing runtime: boardgame.io for turn-based rules, or Colyseus for realtime Rooms/state sync with private fields and reconnect. Consider Nakama when the project also needs its full backend (accounts, storage, matchmaking, social services) and accepts custom authoritative match code plus more infrastructure.

Retain a custom WorldStream runtime only when its explicit guarantees are requirements rather than aspirations: durable/replayable canonical history, deterministic Activity Pack revision pinning, privacy as a protocol-level Projection, typed offers with lifecycle semantics, durable timers, and separately governed external Runners. Those boundaries can justify the extra implementation; ordinary WebSocket rooms, server authority, state sync, reconnect, and basic persistence do not by themselves.

## Browser engine recommendation

For the proposed character-driven, navigable 2D mission, use **Phaser with
TypeScript**, retaining DOM/React for dialogue input, accessible controls,
menus, and account/platform surfaces. Phaser documents browser-focused 2D
rendering, JavaScript/TypeScript development and React integration; its Scene
system includes input, cameras, animation, sound, asset loading and optional
physics. These are useful building blocks, not proof that the mission will be
fun ([introduction](https://docs.phaser.io/phaser/getting-started/what-is-phaser),
[Scenes](https://docs.phaser.io/phaser/concepts/scenes)).

**PixiJS remains a good alternative** if the actual design is an illustrated
operations scene, interactive objects and evidence cards, without much world
navigation. It provides a 2D renderer, scene graph and pointer/touch interaction;
we would assemble more of the game structure ourselves. Its ecosystem includes
React integration. Do not mistake it for a multiplayer server
([PixiJS introduction](https://pixijs.com/8.x/guides/getting-started/intro)).

**LittleJS is capable, not a toy.** Its MIT-licensed engine includes rendering,
physics, particles, audio and input. My preference for Phaser here is about
using its scene-oriented structure for this particular project, not a claim
that LittleJS cannot build it or has worse measured performance
([LittleJS repository](https://github.com/KilledByAPixel/LittleJS)).

Do not mandate one browser engine for every Activity Pack. ADR 0017 already
allows independent, experience-rich Activity Clients. A Phaser client, a
Python agent and a DOM-only accessible client can use the same authorized
Room protocol without sharing rendering code
([ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md)).

## What belongs in each layer

| Layer | Owns | Does not own |
| --- | --- | --- |
| Phaser/React Activity Client | Scenes, characters, input, sound, local animation, readable dialogue | Hidden truth, accepted actions, scores |
| Activity Pack | Mission facts, role permissions, legal operations, discoveries, costs, outcomes | Provider calls or browser rendering |
| WorldStream | Ordered acceptance, authorization, durable history, timers, authorized delivery | A universal NPC brain or physics engine |
| External Runner | Observe, plan, use allowed tools, submit proposals under its own Membership | Authority to declare an outcome or bypass Pack checks |

For example, a person taps a portrait and asks an agent to investigate an
invoice discrepancy. That assignment must reach the real Runner. Its searches
and inspected evidence determine its next proposal. The Pack validates the
resulting action, WorldStream commits and delivers authorized facts, and the
client visualizes them. A confident conversational reply is not proof that a
door opened or evidence was found. NPC dialogue must also respect the NPC's
knowledge and permissions; a model must not invent accepted clues.

The browser can animate walking between server-accepted locations without
persisting each rendered frame. If travel time, collision or line of sight
affects success, the server-side rules must determine those facts. Client
animation must not become arrival authority. A twitch-action physics game
would need a separate suitability investigation; this review recommends a
tactical/investigative game paced around decisions and observable consequences.

## Actual local fit and gaps

The inspected committed baseline was `c5ecd56`. Relevant implementation leads:

- `crates/worldstream-server/src/sqlite_backend.rs`,
  `commit_authorized_participant_action` call: the live SQLite action path uses
  the store's authorized commit method.
- `crates/worldstream-sqlite/src/lib.rs`,
  `commit_authorized_participant_action` and `gateway_room_snapshot`: that
  committed path obtains a recovered Room trace before preparation, with
  additional integrity/history inspection. This is a concrete hot-path cost
  to measure; it is **not** proof of a latency limit or a claim that every
  request necessarily replays from Genesis.
- Active uncommitted changes include `RoomTraceCacheV1` and storage/server
  adapter changes. They aim to retain bounded, storage-fenced execution state.
  They were not changed or qualified by this research. Do not treat them as
  released performance evidence.
- `crates/worldstream-studio-supervisor/src/house_model.rs` builds a structured
  JSON model-response request; a regression explicitly checks that its request
  has no `tools` field. A real multi-step investigation tool loop is not supplied
  merely by choosing a renderer.
- `clients/agent-heist-web/src/webmcp.ts` exposes read, wait and plan-commit
  tools. That is not yet a complete investigation tool surface.
- `packs/midnight-archive/src/companions.ts` and ADR 0028 supply a useful
  bounded-companion-plan seam. Archive's player-committed turns are not the same
  design as a new wall-clock mission; changing that gameplay requires an
  explicit Pack design/revision, not a renderer change
  ([ADR 0028](adr/0028-record-bounded-companion-plans-in-activity-state.md)).

Hosted BYO remains an end-to-end qualification item: browser seat invitations
and a local SDK do not establish a seamless hosted headless-agent onboarding
flow. This review did not exercise that flow and does not certify it.

Do not attribute UI friction or all deployment failures to the kernel. The
September 10 reliability investigation reproduced separate website
maintenance/polling/routing faults and did not establish the original incident
as a Fly crash or SQL defect ([reliability note](room-service-reliability.md)).
No current concurrency capacity, sustained frame rate or provider latency is
established by this review. Measure submit-to-committed-view latency separately
from agent response time, and include active-room load, reconnect, memory and
database growth on the intended small hosting profile.

## Avoid another cosmetic prototype

The next useful experiment is one small playable mission, not a platform
rewrite or another set of canned character progress bars:

1. One interactive location and one verifiable objective.
2. Two actual tool-using agents working on partially known, related evidence.
3. A human can inspect, supply evidence, redirect, cancel or authorize a risky
   step. Their intervention changes actual agent behavior and consequences.
4. One deadline and a disclosed bounded model allowance. Game resources are
   separate from billed token accounting; BYO compute is not automatically
   comparable to House compute.
5. A short result explanation traces success/failure to accepted decisions.
   It does not grade dialogue quality as objective mission success.

Test whether players can explain their objective, whether a specific
intervention mattered, and whether they want to retry without prompting. Also
test the counterexample: can an unchanged generic instruction to both agents
solve everything while the human waits? If so, the intended human-director
gameplay has not been demonstrated. Ordinary pathfinding and animation do not
need LLM calls; intelligence should be spent on investigation and adaptation.

Keep WorldStream for this bounded experiment only if the Pack/Runner/client
seams let us build it without mission-specific kernel branches. If extending
those seams dominates the gameplay work, compare the *same mission* against
Colyseus before investing in more generic infrastructure. Do not run two
authoritative room engines for the same mission as a hedge.

The defensible project pitch is an observable human-agent game with meaningful
steering and consequences. A novel networking engine is neither established
by this review nor necessary for that product to be worthwhile.

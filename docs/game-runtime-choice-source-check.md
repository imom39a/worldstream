# Game client and runtime: source check and recommendation

Research date: September 11, 2026. This is a primary-source check, not an
architecture decision, implementation exercise, or performance comparison.
Where a reviewed source does not document a capability, the finding is scoped
to that source; it is not a claim that custom code cannot supply it.

## Bottom line

WorldStream is reinventing familiar multiplayer machinery if the claim is only
“authoritative rooms over WebSockets with scoped state and reconnect.” Colyseus,
Nakama OSS, and boardgame.io already cover much of that space under permissive
licenses. The defensible custom boundary is the combination of durable
Canonical History, exact Activity Pack Revision pinning, authorized
per-Membership Projections and Replay, durable timers/activation, integrity
verification, and separately governed external Runners. Real tool-using agents
remain application/runtime integration work in all three alternatives.

| Option | Confirmed overlap | Important distinction for WorldStream |
| --- | --- | --- |
| **Colyseus** | Node/TypeScript authoritative `Room`s; clients request mutations and the server synchronizes schema deltas. WebSocket is the documented default transport. `StateView` filters fields/instances per client, and `allowReconnection()` holds a seat and restores a full current-state snapshot. ([state sync](https://docs.colyseus.io/state), [StateView](https://docs.colyseus.io/state/view), [reconnection](https://docs.colyseus.io/room/reconnection)) | Closest drop-in conceptual alternative. Colyseus 0.18 now has an official Drizzle-based database package and versioned cloud saves, so describing all persistence as purely bring-your-own is outdated. However, those docs do not specify an immutable, hash-linked Room transition lineage or historical viewer-authorized Replay. Its “determinism/replay” documentation concerns client-prediction rollback, not durable audit replay. ([database](https://docs.colyseus.io/database), [prediction determinism](https://docs.colyseus.io/netcode/determinism)) |
| **Nakama OSS** | Server-authoritative matches run custom fixed-tick handlers, validate messages, retain isolated in-memory match state, and explicitly broadcast to all or selected presences. Authoritative matches may reserve a disconnected player’s seat; passive-turn guidance persists state through Nakama storage. ([authoritative multiplayer](https://heroiclabs.com/docs/nakama/concepts/multiplayer/authoritative/), [multiplayer overview](https://heroiclabs.com/docs/nakama/concepts/multiplayer/)) | Per-recipient privacy is possible through explicit recipient broadcasts, but the reviewed docs do not define a projection abstraction equivalent to WorldStream’s. Persistence is a developer-chosen state-save pattern, not documented canonical transition Replay. The embedded TypeScript/Lua/Go runtime can integrate services, but external agent capability boundaries, durable attempts, tool loops, and Action Offer semantics remain custom. ([runtime framework](https://heroiclabs.com/docs/nakama/server-framework/introduction/)) |
| **boardgame.io** | A remote game master is the single authority: clients emit moves/events over Socket.IO/WebSockets and the master computes and broadcasts state. Storage adapters persist state/logs. `playerView` strips secrets per player or spectator, and the engine advertises phases, bots, logs, and earlier-state “time travel.” ([multiplayer](https://github.com/boardgameio/boardgame.io/blob/main/docs/documentation/multiplayer.md), [secret state](https://github.com/boardgameio/boardgame.io/blob/main/docs/documentation/secret-state.md), [storage](https://github.com/boardgameio/boardgame.io/blob/main/docs/documentation/storage.md), [README](https://github.com/boardgameio/boardgame.io)) | Strongest fit for a conventional turn-based JavaScript game. `playerView` is a real server-side privacy hook, not merely a UI convention. Its stored log/time travel should not be upgraded into a claim of immutable cross-version, hash-verified Replay without further evidence. Built-in bots are not external agents with real tools or capability-scoped execution. |

Licensing is permissive: Colyseus and boardgame.io are MIT
([Colyseus](https://github.com/colyseus/colyseus/blob/master/LICENSE),
[boardgame.io](https://github.com/boardgameio/boardgame.io/blob/main/LICENSE));
Nakama is Apache-2.0
([license](https://github.com/heroiclabs/nakama/blob/master/LICENSE)).

## Correction and adoption caution

The existing alternatives note is directionally sound, with three corrections:

1. Link Colyseus `StateView` to the current 0.18 page and acknowledge its new
   official database layer; still distinguish saves/current state from
   WorldStream Canonical History.
2. Treat Colyseus prediction rollback “replay” and boardgame.io log time travel
   as narrower mechanisms, not counterexamples to the durable Replay distinction.
3. boardgame.io is not archived and received substantive maintenance commits in
   August 2026, so calling it abandoned would be wrong. Its latest stable npm
   release is nevertheless still `0.50.2`, published November 2022; prototype
   against the published package and verify dependency/runtime compatibility
   before adopting it. ([recent commit](https://github.com/boardgameio/boardgame.io/commit/5e9a2c94bde803fae8b081958c406c4d0a7be8ae), [npm 0.50.2](https://www.npmjs.com/package/boardgame.io/v/0.50.2))

## Practical recommendation

For the hobby tactical investigation—decision-paced, two real tool-using agents,
not 60 Hz physics—keep WorldStream for one bounded vertical slice only if its
Pack/Runner/client seams make durable, private, inspectable agent consequences
cheap to express. Build the same thin mission in Colyseus if kernel work starts
dominating gameplay work. Choose boardgame.io instead only if the design settles
into conventional turns and its published-package spike passes; choose Nakama
when its broader accounts/storage/matchmaking backend is wanted, not merely for
an authoritative match loop.

## Browser client choice

Recommendation, not a platform-wide requirement: **Phaser with TypeScript** for
a navigable, interactive 2D mission, with DOM/React for readable dialogue,
text entry, accessible controls, and ordinary site pages. Phaser supplies scene
lifecycle, cameras, input, asset loading, animation and sound; it supports web
development in JavaScript/TypeScript and integration with React
([introduction](https://docs.phaser.io/phaser/getting-started/what-is-phaser),
[Scenes](https://docs.phaser.io/phaser/concepts/scenes)).

If the design instead stays within illustrated locations, clickable objects
and an evidence board, PixiJS is a reasonable smaller conceptual choice: its
core is rendering, a scene graph and interaction, with more game structure left
to the application ([PixiJS introduction](https://pixijs.com/8.x/guides/getting-started/intro)).
LittleJS is also capable: its engine includes rendering, physics, particles,
audio and input. Preferring Phaser here is a judgment about scene-oriented
authoring, not a benchmark or a claim that LittleJS is unsuitable for serious
games ([LittleJS repository](https://github.com/KilledByAPixel/LittleJS)).

None of these choices requires WorldStream to become a graphics engine or
every Pack author to use the same framework. Existing
[ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md) explicitly
allows protocol-thin, experience-rich independent Activity Clients.

The responsibilities remain separate:

| Layer | Responsibility |
| --- | --- |
| Activity Client, using Phaser and DOM | Present characters, scenes, dialogue and the participant's authorized knowledge; collect intent and animate accepted consequences. |
| Activity Pack | Define mission facts, tools/actions, permissions, resources, deadlines and outcomes. |
| WorldStream | Authenticate and order accepted changes, commit durable state, deliver authorized views and retain Replay evidence. |
| External Runner | Operate an agent through allowed observations and actions, with bounded provider use outside deterministic Pack execution. |

A browser can draw 60 frames per second without creating 60 durable Room
transitions per second. Walking animation can be local presentation. Arrival,
travel time, collision or line of sight must be server-authoritative wherever
they affect the rules. A twitch-action game requires a separate simulation and
networking suitability test; the present recommendation is decision-paced.

## Local evidence and limitations

Committed baseline inspected: `c5ecd56`. This was code/document review, not a
live load test. The worktree contains unrelated storage/cache changes, which
were neither altered nor qualified by this investigation.

- The committed SQLite action path calls `gateway_room_snapshot`, which calls
  recovery and inspection before preparation. This is a concrete hot-path cost
  to measure, not proof of a particular bottleneck or full Genesis replay on
  every request. Inspect `crates/worldstream-sqlite/src/lib.rs` at the baseline
  commit, functions `commit_authorized_participant_action` and
  `gateway_room_snapshot`; the server calls that path in
  `crates/worldstream-server/src/sqlite_backend.rs`. Pending `RoomTraceCacheV1`
  changes must not be reported as released performance evidence.
- The current House provider request asks for one structured `offer_id` and
  `payload`, disables streaming, and omits provider tools. See
  [request construction](../crates/worldstream-studio-supervisor/src/house_model.rs#L1647)
  and its no-tools regression. Repeated action selection can still be agentic;
  this does not make existing agents fake. It does mean a richer multi-step
  investigation, interruption and replanning experience is additional Runner
  and Pack integration, not a renderer feature.
- Heist's browser agent surface currently exposes read, wait and commit-plan
  tools, not a complete investigation toolkit. See
  [WebMCP tools](../clients/agent-heist-web/src/webmcp.ts#L438).
- The shared browser package describes itself as internal, not a general
  public SDK ([README](../sdk/typescript-client/README.md)). SDK existence does
  not certify seamless hosted BYO onboarding; qualify that end-to-end path.
- The documented 100-transitions/second aggregate and sub-100-ms local p95
  targets use a 4-vCPU/8-GiB reference profile. They are explicitly targets,
  not measured capacity of the small Fly deployment
  ([performance envelope](architecture.md#reference-performance-envelope)).

Measure command submission to accepted visible change separately from model
response latency. Include increasing Room history, a small number of concurrent
missions, reconnect, memory, and database growth. Changing engines without
these measurements would not establish the cause of slow or unreliable play.

## What the next playable experiment must prove

Build one small location, two real agents, one verifiable objective and one
deadline. The player can interact with the scene, give an agent evidence,
change priorities, cancel work or approve a risky action. That instruction must
alter actual tool use and accepted consequences, not a prewritten success
animation. The environment can be authored; the agent's solution path need not
be scripted. Conversational claims must not manufacture evidence or outcomes.

Use a consistent visual style, clear interaction feedback and concise agent
reports. Keep plain-language dialogue, but do not require typing for every
routine action. Keep game resources separate from billed model allowance.
Judge the prototype by whether a new player understands the objective, can
identify a consequential intervention, and wants to retry—not by artwork count
or the volume of generated dialogue.

Retain WorldStream only if the mission fits its existing boundaries and feels
responsive on the intended hosting profile. If authoring requires repeated
game-specific kernel changes, or integration work overwhelms gameplay work,
compare the same slice in Colyseus before expanding the platform. Do not add a
second authoritative room engine alongside WorldStream for the same match.

No runtime changes, migrations, deployments, paid model calls, or newly accepted
architecture decisions were made by this research.

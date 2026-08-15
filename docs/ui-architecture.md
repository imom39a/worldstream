# UI and Visualization Architecture

## Status

The frozen releases use a small first-party web console and two first-party reference views. WorldStream remains headless: every capability needed by a participant or runner is available through HTTP/WebSocket and the SDK.

There is no View Pack ABI, generated dashboard system, pack-supplied JavaScript, or runtime LLM-generated UI in v0.1 or v0.2.

## Product principle

> The Activity Pack defines meaning, WorldStream exposes authorized projections and typed actions, and the first-party client renders only the two reference experiences.

This keeps the project focused. A general dynamic UI framework would become another product before the Room Kernel is validated.

## Audience views

The same room can expose different projection schemas:

### Public

- safe for spectators;
- delivered through an explicit read-only spectator membership with its own cursor in the frozen releases;
- excludes all participant-private state;
- uses a token-scoped demo spectator membership; anonymous streaming is not required;
- is the default source for screenshots and demos.

### Participant

- bound to an authenticated participant Membership whose Principal is human or agent;
- contains that role's authorized current state, legal actions, and deadlines;
- may submit the same typed actions as another SDK client;
- never relies on browser code for authorization.

### Operator membership

- local administrative and diagnostic view;
- shows Room Integrity State/generation, complete Head and three state hashes, Membership/Session state, Runner availability, Activation status, timers, incidents/repairs, and Replay verification;
- may show pack-defined debug fields only under explicit operator-membership scope;
- never displays provider credentials or chain-of-thought.

Replay first requires present authorization, then uses the reconstructed Membership/Access/Role at the selected sequence for historical visibility. A later Role or replacement Member ID inherits no earlier private view. Completed Replay may use a separately authorized pack-defined final-reveal Projection; Replay itself does not bypass visibility.

## Runtime data flow

~~~mermaid
flowchart LR
    E["Committed transition"] --> PP["Pack public projection/delta"]
    E --> MP["Pack participant projection/delta"]
    PP --> WS["Scoped WebSocket frame"]
    MP --> WS
    WS --> UI["First-party web client"]
    UI --> ACT["Typed action"]
    ACT --> V["Server authentication and pack validation"]
    V --> E
~~~

The web client never subscribes to raw Authoritative Room State. The audit timeline uses already authorized Domain Events and Observation metadata.

## Shared console shell

Both reference views reuse:

- server and room connection status;
- live versus replay mode;
- room sequence and Membership cursor;
- Core Room Status, operational Room Integrity State/generation, pack-defined Activity Phase, Outcome, and pinned Activity Pack revision;
- Room Member list with Principal kind, Access Mode, Role when acting, Membership status, and temporary session state;
- agent runner availability and activation status shown separately;
- current phase and deadline;
- legal-action form generated from first-party typed definitions;
- public/authorized event timeline;
- activation-reason markers;
- replay sequence slider and Core/Activity/aggregate/lineage hash-verification status;
- error, resync, and slow-consumer status;
- host-operator-only restart/recovery diagnostics.

The shell maps stable protocol data to explicitly authored React components. It is not a generic low-code builder.

## Agent Heist view

### Live public screen

- a small three-route facility map;
- current phase and countdown;
- three role cards and connection/activation indicators;
- public clue claims;
- public plan cards and endorsements;
- public challenges;
- public outcome timeline;
- no private clue, offer, or sealed commitment.

The map can be SVG. There is no physics, 3D engine, avatar system, or free-roaming world.

### Human participant screen

When a human occupies a player role:

- role-specific clues;
- structured clue exchange inbox;
- plan proposal and endorsement controls;
- sealed commitment form;
- personal deadline and legal actions.

These controls send ordinary action.submit messages.

### Operator-membership screen

- all membership, session, runner, timer, frame-cursor, and activation state;
- scripted failure-injection controls in development builds;
- room fault and storage diagnostics;
- no model private memory or chain-of-thought.

### Replay screen

- read-only sequence slider;
- public historical projection;
- each participant historical view only when authorized;
- completed final-reveal view;
- Core, Activity, aggregate Authoritative State, and Transition-lineage hash status;
- exact rule explanation for outcome.

## Investigation Room view

### Case header

- Cold Chain Incident objective;
- current phase and deadline;
- participating human and agent roles;
- pending assignments, reviews, stale claims, and final-brief readiness.

### Evidence panel

- immutable evidence cards with digest abbreviation, label, version, media type, provenance, assignment, and visibility;
- explicit supersedes/superseded marker;
- safe text/image preview or forced attachment download;
- no arbitrary HTML or document execution.

### Fact and claim board

- published facts with exact evidence links;
- claim revisions;
- support and challenge edges;
- verification disposition;
- stale/withdrawn/superseded state;
- filter by current, challenged, or stale.

A small node-edge graph MAY complement the accessible list. The list remains the primary interaction surface.

### Timeline

- evidence releases;
- published facts and claims;
- verification requests/results;
- correction arrival;
- deterministic invalidation chain;
- activation reasons;
- final brief and score.

### Human Lead controls

- assign evidence;
- request verification;
- review current claim set;
- submit the structured final brief.

Every form maps to a typed Activity Pack action. The UI does not infer a business workflow.

### Final brief

- conclusion code;
- ordered evidence-linked timeline;
- selected claims;
- contradictions addressed;
- uncertainties;
- confidence;
- deterministic rubric breakdown.

## UI state model

Client state is divided into:

### Server-backed durable state

- current authorized projection;
- observation cursor;
- room and frame sequences;
- replay response;
- accepted/rejected action receipts.

The client may retain Room Integrity State/generation and complete Head only as server-backed operational/causal metadata. Neither is rendered as Activity State or editable client truth.

### Temporary transport state

- socket connecting/connected/disconnected;
- last heartbeat;
- pending request IDs;
- reconnect backoff;
- local optimistic form state.

### User preference state

- selected tab;
- panel sizes;
- filters;
- color theme.

User preferences may use browser storage. They are not room state and do not enter replay.

The client SHOULD avoid optimistic domain mutation. It may show an action as submitting, but canonical UI state changes only after the accepted reply and committed observation.

## Reconnect behavior

1. Store the last durably rendered frame Cursor.
2. Reconnect and attach with that Cursor.
3. Atomically install the complete retained range through the captured frame head, or replace the Projection from a full Reset.
4. Acknowledge the captured baseline with this Session's sync token before entering Live.
5. Render later/redelivered frames idempotently; another Session's shared-Cursor acknowledgement never satisfies this Session's barrier.
6. Clearly label disconnected, attaching, catching-up, live, faulted, quarantined, and replay modes.
7. Never show stale controls as currently legal while catch-up is incomplete. A stable stale Action requires synchronization and a new Action ID if it remains legal.

An Agent Participant's separately authorized Room Member client uses the same Cursor semantics through the SDK even if it has no visual UI. The Runner activation-control client does not own or acknowledge that Cursor.

## Integrity and recovery behavior

- Loading/CatchingUp does not render stale data as current or enable normal Action controls.
- Faulted renders a persistent banner with state, generation, last verified complete Head, and safe reason code; disables all canonical mutation; and may show only the last verified authorized Projection, retained Catch-up, or verified Replay.
- Quarantined removes normal Projection, Catch-up, Action, and claimed-current Replay surfaces. An authenticated host-operator view may expose bounded diagnostics, raw-export/restore requests, verification progress, and incident history without raw pack-private state.
- The UI may request repair but cannot clear integrity or edit canonical history. Only a successful generation-fenced verifier result changes the displayed state to healthy.
- Archived is distinct from unhealthy: a healthy archived Room remains readable/replayable and permits only authorized suspend/depart administration.

## Component choices

Frozen frontend:

- React and TypeScript;
- Vite build;
- native browser WebSocket;
- first-party CSS/component styling;
- SVG for Heist map and optional Investigation graph;
- no server-side rendering requirement;
- static assets embedded in or served beside worldstreamd.

Avoid adding a large state framework until real complexity requires it. A small reducer/store can model connection, projection, timeline, and replay state.

## Security boundary

- All text is untrusted and escaped.
- Markdown, if displayed later, uses a strict sanitizer and disabled raw HTML.
- URLs are not made clickable without an allowlisted scheme.
- Artifact preview never interprets scripts, active SVG, office macros, or embedded HTML.
- Authentication tokens stay in memory or secure local development storage and never enter URLs.
- WebSocket origin is validated.
- The UI cannot request another membership's view by changing a client-side ID.
- Controls shown beside an Operator Membership view require a separate host-operator capability; the Operator Membership itself remains read-only.
- Pack data cannot choose component types, CSS, event handlers, or network endpoints in the frozen releases.

## Accessibility and testability

- keyboard-accessible forms and timeline;
- visible focus state;
- status not expressed by color alone;
- live updates announced without flooding screen readers;
- countdowns include text and do not update accessibility announcements every second;
- graph information has an equivalent table/list;
- stable test IDs only where semantic roles are insufficient.

Required UI tests:

- private data never appears in public state or rendered DOM;
- duplicate frames do not duplicate timeline items;
- projection reset removes stale private state;
- reconnect state disables stale actions;
- stored text cannot execute HTML/script;
- Heist and Investigation actions produce valid protocol payloads;
- replay never enables mutating controls.

## External clients

A specialized game, terminal UI, mobile app, or enterprise front end may ignore the first-party console and use the same scoped protocol.

External clients receive no special database access. Their actions remain typed and server-validated. This is the extension path for visualization until there is evidence for a reusable View Pack contract.

## Deferred presentation work

After both reference releases, an RFC may evaluate:

- a safe declarative view schema;
- reusable table, board, graph, timeline, chart, and form primitives;
- a sandboxed custom-renderer boundary;
- external-client discovery metadata;
- authoring-time AI assistance that emits validated declarative configuration.

Runtime LLM generation of executable UI is not a target. It would make permissions, replay, accessibility, tests, and a stable user experience harder precisely where the project needs clarity.

## Required invariants

1. The server is fully usable without the web client.
2. The UI consumes only authorized projections and frames.
3. Humans submit the same typed actions as agents.
4. Membership, session, runner, and activation status are shown separately.
5. Live, disconnected, catching-up, and replay state are unmistakable.
6. Canonical UI state changes only after committed server output.
7. Public DOM never contains private projection fields.
8. Replay is read-only and visibility-aware.
9. Activity data cannot execute code or choose arbitrary rendering behavior.
10. Investigation adds first-party components, not a generic UI platform.
11. Faulted and quarantined serving surfaces remain visibly and behaviorally distinct.
12. No UI control can mark a Room healthy or rewrite Genesis/Transitions.
13. Complete Head and all three state hashes are displayed as metadata, never Core or Activity State.

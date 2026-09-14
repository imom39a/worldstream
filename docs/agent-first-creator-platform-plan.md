# Agent-first creator platform: a practical plan

Date: 2026-09-11. Source baseline: `e1c3817`.

Status: researched proposal for implementation planning, not a new frozen
protocol, accepted ADR, release certificate, or deployment report. The user
has set the product direction: keep Agent Heist as a technical proving ground;
build a separate player-facing game and make creation and sharing practical.
The recommendations below do not silently supersede existing ADRs.

## 1. The direction

Build a creator platform for challenges that humans and independently operated
agents can play, direct, collaborate on, compete in, or watch. WorldStream is
the underlying authoritative Room Runtime, not the name for every experience
and not a model-hosting service.

The promise is: **Create a challenge. Bring an agent or use a House Agent.
Share it with friends. See what their decisions accomplish.**

Separate the work into three deliverables:

| Deliverable | Its job | Success evidence |
| --- | --- | --- |
| Existing Agent Heist | Technical demo/conformance Pack; exercise streams, privacy, timers, simultaneous Actions, recovery and results. | Reproducible correctness, failure and performance tests. |
| New flagship Pack | An engaging, historically inspired fictional heist with a graphical Activity Client and real agent work. | New players understand it, influence agents and want another attempt. |
| Creator starter and sharing path | Help a developer build a different challenge and invite other people. | An outside developer succeeds without a kernel patch or maintainer operating their machine. |

This is consistent with [ADR 0014](adr/0014-installable-wasi-free-activity-pack-bundles.md),
which already classifies Agent Heist as demo/conformance work. Preserve its
existing identities, fixtures and retained Rooms. Do not rename it in place,
rewrite its history or require the new gameplay to fit its existing rules.
Public presentation can later distinguish **Lab/demo** from **Playable games**.

Midnight Archive and Negotiate remain useful reference implementations. Reuse
their proven components and tests; do not silently change their rules or
declare either to be the new flagship.

The Roblox analogy is a long-term creator relationship, not an MVP feature
checklist. We do not need an economy, 3D editor, global matchmaking or an
unrestricted code-hosting service to prove this relationship.

## 2. Research conclusions

Supporting notes:

- [Creator platforms, agent interfaces and evaluation](agent-first-creator-platform-research.md).
- [Historical inspirations and game concept](historical-heist-game-research.md).
- [Renderer and multiplayer alternatives](game-runtime-choice-source-check.md).

The useful lessons are:

1. Make local testing and audience selection understandable. Roblox documents
   distinct testing and publication steps; private publication is not the same
   as granting playtest access. Borrow that clarity, not its whole platform
   architecture. [Roblox publication](https://create.roblox.com/docs/production/publishing/publish-games-and-places)
2. A small browser-game project can have a simple upload/preview experience.
   itch.io demonstrates that ergonomics, but uploading browser assets is not
   equivalent to approving server-side rules or safely hosting arbitrary code.
   [itch.io HTML games](https://itch.io/docs/creators/html5)
3. Agent interfaces need explicit observations, Actions and terminal behavior,
   plus executable tests. PettingZoo and TextArena are useful interface
   precedents, not replacements for WorldStream authority.
   [PettingZoo](https://pettingzoo.farama.org/api/aec/),
   [TextArena](https://github.com/TextArena/TextArena)
4. Remote MCP and WebMCP solve different integration problems. A shared game
   URL does not automatically connect an arbitrary assistant. Remote clients
   need supported configuration and authorization; browser tools need a
   supporting browser/agent with the page open.
   [MCP transports](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
   [WebMCP draft](https://webmachinelearning.github.io/webmcp/)
5. Historical inspiration supplies a theme, not proof of enjoyable gameplay.
   The Gardner Museum's 1990 theft offers a strong missing-art/recovery theme.
   Use an invented setting and case, rather than claim to solve its ongoing
   real investigation. [Museum history](https://www.gardnermuseum.org/about/theft)

These sources validate patterns and factual inputs. They do not establish
market demand, novelty, retention, model quality or performance of our product.

## 3. What exists, and what is missing

This is a source audit, not a new live acceptance run.

| Area | Evidence in this repository | Next requirement |
| --- | --- | --- |
| Room authority | Core/storage/server implementation; per-Membership views and offered Actions; retained Replay. | Keep technical regression coverage independent of game polish. |
| Runtime performance work | `e1c3817` includes bounded Room execution caching, delivery and observation-retention changes. | Measure the committed candidate on the small hosting profile. Do not repeat the earlier note's description of this as uncommitted work or claim it is deployed. |
| Non-Rust Pack authoring | TypeScript SDK, compiler/bundler, five-operation WASI-free Component contract. | Prove a fresh creator installation, not just a monorepo build. |
| CLI | `new`, `create`, `check`, `test`, `build`, `inspect`, `prove` exist. | Add the missing complete-game development and submission workflow. |
| Package availability | Unauthenticated npm registry lookups for `@worldstream/pack-cli` and `@worldstream/pack-sdk` both returned HTTP 404 during this review. | Do not advertise those `npx` commands as a working public installation. Publish qualified alpha tooling or supply a tested pinned source/distribution path. |
| Prompt assistance | `prompt.ts` only supplies a name/description for a fixed two-party negotiation starter. | A game/scenario assistant is new work. Keep output reviewable and under the same tests. |
| Browser clients | Independent Heist, Archive and Negotiate clients plus shared transport primitives. | Package a usable client integration for creators; the TypeScript client README calls it internal. |
| First-party client hosting | `serve-activity-clients.mjs` explicitly mounts retained first-party builds. | Derive reviewed mounts/build inputs from Distribution metadata; do not describe this as a generic creator host today. |
| House execution | Bounded provider request/allowance path and Archive companion plans. | Define and implement a bounded multi-step game-tool loop; current provider request has no tools. |
| BYO | Python protocol client, local assignment MCP and narrow Heist WebMCP; hosted external-seat correspondence exists. | Prove end-to-end hosted agent admission, connection, revocation and recovery from another machine. |
| Sharing/results | Listing visibility, invitations, Run entry and reviewed result-projector contracts. | Expose a coherent creator preview flow; schema fields alone are not a functioning private creator service. |

Code leads: [Pack CLI](../sdk/typescript-pack/packages/pack-cli/src/main.ts),
[scaffold](../sdk/typescript-pack/packages/pack-cli/src/scaffold.ts),
[prompt assistance](../sdk/typescript-pack/packages/pack-cli/src/prompt.ts),
[Pack SDK](../sdk/typescript-pack/packages/pack-sdk/src/index.ts),
[browser SDK](../sdk/typescript-client/README.md),
[House model adapter](../crates/worldstream-studio-supervisor/src/house_model.rs),
[Room cache](../crates/worldstream-core/src/room_trace_cache.rs),
[Archive candidate limits](midnight-archive-hosted-candidate.md).

The Archive candidate specifically does not currently permit BYO, arbitrary
prompts or human invitations. Its local qualification is not proof of the
proposed creator experience. The formal Starter release also has separate
unresolved qualification requirements; a working hobby deployment is not a
certified Runtime release. [Starter status](starter-distribution.md)

## 4. Architecture to preserve

```text
Creator source project
  └─ local checks/build
       └─ Activity Distribution proposal
            ├─ headless Activity Pack Bundle
            ├─ independent Activity Client Release
            └─ compatible integration references and documentation
                 └─ separate Host approvals + platform Listing review

Human browser ── Phaser/DOM client ──────────────┐
House Runner ── scoped participant client ──────┼─ Hosted Gateway ─ WorldStream
BYO agent ── SDK or scoped protocol adapter ────┘                      │
                                                                      └─ Pack rules
Website/BFF ── sign-in, formation, invitations, catalog, derived results
```

This shows logical ownership, not identical network routes. Browser streams
use existing Host-issued admission tickets. A proposed remote MCP adapter uses
HTTP for its client-facing protocol and connects to the same Room authority;
it does not introduce another authoritative stream server.

| Owner | Owns | Must not own |
| --- | --- | --- |
| WorldStream | Membership, ordered acceptance, durable Room facts, timers, authorized delivery and Replay. | Model calls, game-specific UI, a creator marketplace or arbitrary web fetches. |
| Activity Pack | Mission facts, Roles, legal Actions, discoveries, resources, phase progression and Outcome. | Provider credentials, model execution or browser rendering. |
| Activity Client | Graphical scenes, input, dialogue presentation and authorized state display. | Hidden truth, rule enforcement or client-authoritative scores. |
| Runner | Model/tool orchestration, private strategy/memory and bounded invocation execution. | Authority to invent results or act for another Membership. |
| Hosted Activity Platform | Accounts, catalog, reviewed publishing, formation, invitations, derived result presentation. | A second copy of authoritative gameplay state. |

One game session is one Room governed by one Pack. Its gallery, office and
dock are Locations in Activity State, not three Rooms requiring a new
cross-Room protocol. Continue to use exactly `descriptor`, `initialize`,
`reduce`, `view` and `observe`; the new mission is a test of those boundaries.

Agent-first means equal protocol standing, not identical UI or credentials.
Human, House and BYO participants with equivalent Roles and knowledge face
equivalent rules. Exact Action Offers remain Membership/view-bound. Character
abilities belong to the Role; House versus BYO describes who supplies the
intelligence. Owning an agent does not give its human the agent's credential
or private view.

### Data and deployment

Keep the existing single-authority topology for the alpha:

- **Vercel:** website, first-party static game assets and fixed-route BFF.
- **Fly:** one authoritative runtime with its Gateway, Controller and bounded
  external House processes. All WorldStream live WebSockets remain here.
- **Supabase:** existing platform-only identity/coordination and derived result
  records. No gameplay mirror and no Supabase Realtime relay.
- **Fly volume:** durable Room storage, retained exact Pack artifacts,
  Controller state and separate provider-allowance evidence.

Do not migrate the canonical database merely because a new game is being
added. Preserve the selected supported backend; the checked-in hobby topology
uses SQLite, while PostgreSQL remains a supported separate runtime profile.
Record actual deployed configuration during verification rather than infer it
from `fly.toml`.

Use bounded, structured Activity State and participant Projections. Small
fictional records can be Pack material; larger assets are immutable references
with enforced access. A read of already authorized evidence can be local or a
scoped read adapter. Discovering previously hidden evidence, consuming a lab
slot or opening an interview is a Pack Action if it changes knowledge or rules.
An agent's external memory remains external. Do not add a shared vector-memory
service for this first game.

Keep browser/provider secrets out of bundles and client code. New platform
tables, if actually needed, follow the existing private `platform_store` and
server-only `platform_api` pattern. Validate account ownership in the BFF and
RPC path; service-role access bypasses RLS, so RLS alone is not that path's
authorization. Review grants and RLS together.
[Supabase RLS guidance](https://supabase.com/docs/guides/database/postgres/row-level-security),
[existing data boundary](adr/0023-use-supabase-for-platform-coordination-and-replay-verified-results.md)

Start with existing static asset delivery and GitHub-reviewed artifacts. OCI
remains the accepted long-term transport, not a new registry service to build.
GitHub's registry supports OCI artifacts, but actual custom artifact publication,
verification and access must be tested before claiming that path works.
[GitHub registry](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry)

## 5. Make creation and sharing easy

There are three different user jobs. Do not force all of them through the Pack
compiler.

### A. Start a friend's game

Open the reviewed game page, sign in, choose a reviewed roster and available
seat, create a waiting room and invite friends. No Rust, terminal, provider
account or Pack installation. "Waiting room" is friendly UI language for
coordination around one Launch Request before Genesis, not an already-running
WorldStream Room. Roster Options refer to predeclared seats, not arbitrary new
Roles. Each invitation claims one exact seat.

Proposed first-game UX: prominently offer the allowed House fill option, with
BYO as an alternative rather than a setup prerequisite. Use only the Listing's
allowlisted House revisions in eligible unclaimed seats. Preserve explicit
fill-mode selection, freezing after the first Seat Claim, and the final claim
window/fill operation. The richer tool-using specialists below require the
new House execution contract; they are not already available by changing copy.

The game page explains objective, expected duration, controls, publication and
agent spending limits before start. Waiting-room instructions include a short
practice opportunity, not a wall of protocol terms.

### B. Remix a reviewed challenge without code

Proposed alpha path: expose a small set of validated inputs from a reviewed
Pack, such as case variant, allowed deadline range, selected difficulty and
reviewed roster. A saved remix is a launch configuration, not a new runtime or
arbitrary executable Pack. Freeze its resolved inputs for each new Room.

Initially these are reviewed presets and bounded schema inputs. Editing hidden
truth, the evidence graph, scoring or allowed tools is authoring work and needs
the same validation/revision process as code. Prompt assistance can propose a
bounded scenario draft for review; it cannot publish or change a running Room.

An ordinary share URL identifies a game/configuration. A Seat Invitation is a
separate revocable pre-Genesis claim capability, used with sign-in, and still
does not itself grant Room Membership. Unlisted means absent from discovery;
it does not mean access-controlled. Keep watching, joining and public results
as separate policies.

### C. Author a new game in TypeScript

Supply one complete starter with a playable browser client, a deterministic
Pack, two sample agent policies, privacy tests and a debrief. Counter remains a
small teaching fixture; it is not the advertised complete-game starter.

Illustrative source layout, not a required new wire format:

```text
my-challenge/
  pack/                 rules, schemas and private scenario facts
  client/               Phaser/DOM scenes and public assets
  agents/               local example policies; no credentials
  fixtures/             successful, invalid, timeout and privacy journeys
  integration/          Distribution source, Listing/projector proposals
  docs/                 rules, controls, attribution and licenses
  package.json          simple user-facing scripts
  package-lock.json     or the starter's one supported lockfile
```

Keep artifacts separate even when their source travels together. Shared types
must not pull private evidence, answer keys or seeds into the browser build.

The intended UX is below. These scripts are **proposed**, not commands already
provided by today's CLI:

| Creator operation | Tooling responsibility |
| --- | --- |
| Create starter | Write reviewable source and pinned dependencies; offer optional prompt assistance. |
| `npm run dev` | Start an isolated local Host and client, offer one-click role previews and deterministic test agents. |
| `npm run verify` | Run rules, privacy, replay, client contract and production Component checks; explain failures with file/action references. |
| `npm run bundle` | Build exact Pack and client artifacts plus a Distribution proposal; show no raw hashing chores. |
| Submit for preview | Prepare a reviewable GitHub contribution or artifact request; explicitly show pending approval. |
| Share approved preview | Give a game page and explicit invitation controls without another compile/deploy. |

The wrapper composes existing tooling; it does not replace the Component Host
prover with JavaScript-only tests. Local live preview uses the actual runtime.
Fixtures and simulated providers are visibly marked and never pass as real
agent evidence.

Client presentation can hot-reload in isolated development. A rule change
creates a new revision and a new disposable test Room; it does not mutate an
existing lineage. Published updates keep old compatible client artifacts and
Pack revisions available for retained Runs.

### Publishing boundary

For the first creator alpha, accept reviewed GitHub submissions, including
private artifact intake when source must remain private. We operate the
approved preview. Publish once; friends can then launch many fresh sessions
within quotas without asking us to redeploy each one.

New executable submissions still wait for review. We should disclose that
delay rather than imply instant self-service. Build an upload queue, isolated
build workers and automatic client deployment only after outside creators
demonstrate recurring demand. The first improvement is a shorter workflow,
not removal of Host approval.

Keep three decisions separate even when one hobby operator performs all of
them: the Application Integrator proposes a Distribution; the Platform
Operator reviews Listing/admission policy; the Host Operator approves exact
Pack Bundles, Client Releases, Deployments and Bindings. GitHub review does
not grant those approvals by itself. Preserve whether a Client Deployment's
running bytes are verified or it is explicitly externally trusted; approval
alone is not verification.

Current first-party clients share a trusted origin. Do not put arbitrary
creator JavaScript there: it could act through that origin's authenticated
session. Reviewed contributions may become trusted first-party deployments;
untrusted custom clients need a separate origin/sandbox and handoff security
decision. WASM fuel/memory limits do not make browser JavaScript safe or make
the current process a hostile multi-tenant sandbox.

## 6. Agent integration: the smallest real loop

The first real loop should be:

1. Receive the participant's current Projection, objective and Action Offers.
2. Select an allowed query or Action and run it through the scoped adapter.
3. Read the actual result or receipt; do not infer success from a model reply.
4. Revise the plan when evidence, instructions, resources or available Actions
   change.
5. Stop on task completion, cancellation, terminal state or allowance limits.

Package reusable observation/offer handling, tool-result rendering and receipt
handling so every creator does not write a model gateway. Domain tool names
such as `inspect_record` are proposed game-facing conveniences. They must map
to the existing authorized read/Action contract, not grant a parallel mutation
path or add a sixth Pack callback.

Start with a bounded House tool adapter and one tested Python BYO client. A
local stdio MCP bridge can help compatible desktop clients. Add a remote MCP
endpoint after the scoped admission/authentication contract is settled;
Streamable HTTP is that adapter's transport, not a replacement for WorldStream
WebSockets. OAuth authorization does not itself create a Membership. Bind
access to the user's admitted agent seat, deployment and permitted operations;
support revocation without erasing the agent's Room history.

WebMCP remains optional browser enhancement, not the only BYO path or a launch
dependency. Feature-detect and test the current draft/implementation. Do not
promise arbitrary ChatGPT/Codex/browser versions can connect merely by receiving
a URL. Maintain a tested-client compatibility matrix.
[MCP authorization](https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization)

### Constraints that must be designed before implementation

- A changed task invalidates old work. Cancellation must prevent late results
  from acting, even when a provider call cannot be stopped or refunded.
- A stale Head produces a clear rejection and fresh authorized context. A
  prior proposal is never silently rebased into a new decision.
- Multi-step provider attempts need durable step identities and reservation
  accounting. A reconnect/restart must not resend an ambiguously paid call.
- No unattended House shell, internet browsing, third-party connectors or
  access to creator/host secrets. The initial tools are bounded game tools.
- BYO agents may retain external strategy, knowledge and tools on their own
  machine. The server validates their Actions but cannot attest their model,
  spend, private tools or human assistance.
- A prompt inside evidence is untrusted game data, not an instruction granting
  permissions. Provider routing and spending policy are not editable by Packs.
- Report meaningful discoveries, blockers and requested decisions. Do not
  require or expose hidden chain-of-thought to make the agent observable.

Human direction is recorded through Pack-defined task/communication Actions.
For the first game, a Crew Lead Role can allocate shared resources and approve
the final commitment. The first client lets a human hold that Role. A future
agent captain may hold the same Role; it is not a kernel-level human privilege.
An observer remains read-only until explicitly admitted to a suitable Role.

## 7. First player-facing game

Working title: **The Empty Gallery**. Naming is provisional.

Recommendation: a fictional art-recovery heist inspired by the Gardner theft,
not a documentary reconstruction. The museum records thirteen stolen works
and an ongoing recovery effort. Our objects, characters, locations, records and
solution are invented. An exact historical reenactment is a possible later
creative choice, but known facts alone do not supply a replayable agent game.
[Historical research](historical-heist-game-research.md)

### Proposed experience

One Crew Lead and two specialists investigate a collection before a fictional
transfer deadline. Identify the genuine missing object among plausible
matches, establish its provenance, and commit a recovery plan. The initial
full-session target is roughly 12 minutes, to be changed by playtesting.

The Archivist follows catalogue, repair and custody evidence. The Field
specialist tests witness accounts and negotiates access. Both are actual Agent
Participants, supplied as House or BYO. Start cooperative; a rival team and
25-minute competitive variant are later content, not the first milestone.

Make two different evidence routes viable. A lab slot can resolve an ambiguous
object match or authenticate a ledger, but cannot do both before the deadline.
The human can inspect the scene, give an agent a newly observed contradiction,
change priorities, approve a trade or choose a supported early recovery.
The decision is about allocation and risk, not selecting four matching answers.

An agent might follow a repair mark into a record, reject that lead because
its date conflicts, then query custody aliases and request another check.
Changing its instruction or supplying different evidence must change that
real sequence. The useful work must not be a fixed progress bar whose outcome
is always success.

Start with a small authored evidence graph and short records. Do not create
difficulty by forcing people to read hundreds of documents. Challenge comes
from uncertain interpretation, conflicting commitments and limited access.
Use seed-dependent hidden variants within tested bounds; every generated
instance must be solvable under at least one known witness plan. An LLM does
not generate authoritative world truth during the match. Resolve and record
the exact variant inputs or initial hidden state at Genesis under the pinned
Pack Revision. Keep secret inputs out of client bundles and unauthorized
Projections. Replay uses recorded inputs without a provider or fresh external
randomness.

### Graphical client

Use Phaser with TypeScript for one illustrated 2D location and interactive
characters; DOM/React handles readable dialogue and accessible controls.
Scenes, input, sound and animation are supported engine primitives.
[Phaser scenes](https://docs.phaser.io/phaser/concepts/scenes)

The screen shows the place, objective, clock, crew and current consequential
interaction. Click an object to inspect it; click a character to assign or
redirect work. Show evidence as named objects/cards and agent findings as
short reports. Put detailed history in an optional dossier, not the primary
play loop. No raw IDs, long action-form column or mandatory page scrolling.
Provide touch targets, keyboard alternatives, captions and reduced motion.

Ordinary motion/path animation needs no model call. Server rules govern any
movement, arrival or visibility that affects success. The client can animate
smoothly between accepted facts without committing every rendered frame.

### Rules, clock and results

Provide untimed practice before a live timed run. Start the mission timer only
after the reviewed readiness/start condition. In live play, opening help or
disconnecting does not silently pause time. Show a countdown and explicit
execution-degraded state. Provider outages are not hidden as bad agent skill.
The Pack must define no-Action/deadline consequences and visible fallback
choices available to the Lead; no fabricated companion success.

Outcome checks use actual object identity, accepted evidence relationships,
resource use and the deadline. Separate primary recovery success from optional
ledger disclosure, trust and remaining resources. Explain the score or outcome
vector before play and cite accepted decisions in the debrief. Any leaderboard
aggregation comes later; an LLM judge does not establish success.

Use original or properly licensed art, sound and documents. Historical subject
matter is not a blanket permission to copy museum material. Gardner's image
terms have non-commercial/no-derivatives conditions; check each shipped asset
and keep provenance. Do not imply museum endorsement or invent factual claims
about living people. [Museum rights](https://www.gardnermuseum.org/organization/rights-reproductions)

## 8. Evaluation without misleading competition claims

Use three separate labels:

- **Practice:** scripted fixtures, no real-agent performance claim.
- **Open exhibition:** humans, House and BYO setups; celebrate the whole
  system's performance and disclosed assistance.
- **Controlled evaluation, later:** pinned scenarios/versions, roles, tool and
  execution policy, repeated runs and clearly stated comparability limits.

Canonical Replay reproduces recorded consequences. It does not reproduce
hidden model reasoning, prove model weights, or certify external tool use.
Do not identify an external model as verified from a user's submitted name.

For development compare a deterministic heuristic, an unsteered agent and a
steered agent on multiple case variants. Record completion, invalid Actions,
evidence quality, time, provider use where observable and human intervention.
Also test a human-only condition with matched information and Action access
where possible, documenting any Role differences. A tiny seven-record proof
can validate the integration without establishing that the game needs agents.
Do not claim agent necessity from a deliberately handicapped human baseline
or add document volume merely to manufacture that claim.
Use role swaps and cross-play before competitive claims. A fixed heuristic
winning repeatedly is useful evidence to redesign the challenge, not a reason
to artificially cripple it. Small playtests guide design; they are not a
statistically established benchmark or product-market-fit result.

## 9. Cost and operating limits

Do not buy services, change billing, raise model limits or deploy as part of
this plan. Reuse the hobby stack and keep local fixtures the default for
mechanical tests. Real-agent qualification must still make real calls under a
separately approved test allowance.

Before dispatch, reserve the maximum permitted request cost and one attempt.
Bound input/output, calls, concurrent work, tool steps and wall-clock duration.
Ambiguous calls consume the conservative reservation. Game energy/cooldowns
never refill a provider allowance. Keep the dedicated key's external cap and
auto-top-up setting independently verified; do not infer them from this doc.
[OpenRouter key limits](https://openrouter.ai/docs/api/api-reference/api-keys/create-a-new-api-key)

Measure actual cost per completed, failed and abandoned session before choosing
the public quota. Begin qualification with one Run, then two simultaneous Runs
and at most four House agents as an experiment within existing deployment
limits. This is not a capacity promise. Close new House admissions when the
budget is exhausted; preserve in-progress Room truth and explain the impact.

Hosting is not automatically capped by an LLM key. Bound Machines, launches,
connections, asset sizes, payloads and storage retention; do not enable a
Machine-creating autoscaler. Fly bills resources such as volumes and transfer
separately. Supabase cost controls cover specified usage, not every possible
line item. Review actual provider settings before release, rather than promise
zero overage. [Fly cost management](https://fly.io/docs/about/cost-management/),
[Supabase cost controls](https://supabase.com/docs/guides/platform/cost-control)

Keep assets and raw provider transcripts out of canonical transition payloads.
Track bytes per Run, observation retention, database growth and recovery time.
Do not delete immutable history to meet a surprise storage limit. Define and
disclose any future retention policy before making replay promises.

## 10. Delivery sequence and gates

Work estimates below are planning ranges for one experienced developer. They
exclude waiting for outside testers and are not commitments. Re-estimate after
the real-agent slice; model latency and integration quality are unresolved.

| Phase | Deliverable | Exit gate | Focused hours |
| --- | --- | --- | ---: |
| 0. Establish the baseline | Preserve Heist's technical role; record current runtime/deployment evidence and approve only needed contract extensions. | Known source/deployment boundaries and a small executable acceptance checklist. | 6–10 |
| 1. Prove real agent steering | One small fictional case, one real House specialist, two consequential Lead decisions, one deadline; SDK transport seam. | Actual tool branch and a consequential redirection; deterministic rejection of unsupported answers. | 18–30 |
| 2. Make the game playable | One Phaser location, two specialists, practice, full mission and causal debrief. | At least four of five first-time players explain their objective and one consequential decision without coaching; record whether they want another attempt. | 24–40 |
| 3. Complete BYO participation | Account-owned agent-seat admission, scoped connection, Python example/local bridge, revoke/reconnect. | An external agent on a second machine completes the same game under the same rules. | 20–36 |
| 4. Prove creation and sharing | Complete starter, verified installation, dev/verify/bundle wrapper, reviewed preview submission and invitations; bounded assisted drafting. | Two outside creators build distinct challenges, including one with changed rules rather than just configuration; friends play approved previews without per-session operator setup or kernel edits. | 28–48 |
| 5. Qualify the closed alpha | Failure/privacy/load/cost checks and two end-to-end hosted journeys. | Repeat play, reconnect, results and budget gates pass on exact deployed artifacts. | 16–28 |

Total rough range: 112–192 focused hours before contingency. At an illustrative
8–12-hours/week hobby cadence, plan roughly 10–24 weeks, plus buffer. Do not
advertise the whole creator platform as a two-week project. A much smaller
agent-steering proof should arrive during phases 0–1, well before that total.

Draft the starter alongside the new Pack, but extract reusable utilities only
after real duplication. Phases 3–4 may overlap once the game contract settles.
Remote MCP/OAuth product integration is a separate follow-on estimate after
the SDK BYO path works; WebMCP enhancement must not block the alpha. The alpha
must not claim every compatible assistant can join until each advertised path
is actually tested.

### Acceptance matrix

| Journey or failure | Required observation |
| --- | --- |
| Human + House, then Human + BYO | Same game rules; actual Actions and receipts, not fixtures presented as live. |
| Refresh/reconnect | Original Membership and authorized baseline; no duplicate Room or restarted clock. |
| Cancel/reassign while model runs | Late proposal cannot act on the cancelled task; paid reservation is not refunded accidentally. |
| Simultaneous conflicting Actions | One ordered legal result; rejected participant can inspect current authorized context. |
| Private evidence and spectator | Hidden facts absent from network data, initial state, browser bundles and public debrief. |
| Malicious content | Prompt injection cannot expand permissions or access secrets; unapproved code is never executed. |
| Invalid/late/missing agent output | Safe rejection or explicit Pack consequence; no invented success or unbounded retry. |
| Leave/finish/rematch | Capacity reconciles from evidence; a rematch creates a new Run; prior result remains correctly linked. |
| Version update | Old Runs retain their Pack/client/projector compatibility; new rules create new revisions. |
| Slow consumer/history growth | Bounded memory and queues; safe catch-up/reset; measured latency separated from provider time. |
| Fresh creator installation | No unpublished dependency surprise, Rust source edit, manually typed digest or secret-bearing artifact. |
| Repeat hosted play | At least two complete consecutive matches without database cleanup or operator seat repair. |

For the small alpha, propose a separately measured sub-500-ms p95
submit-to-accepted-visible-change target on the tested network/load profile,
excluding provider time. It is a new UX target, not a replacement for the
existing local reference performance targets or an achieved result. Log model
wait separately and redesign pacing if users spend most of the session idle.

Ordinary same-volume restart/re-entry remains a gate. Full populated disaster
recovery remains separately deferred under
[ADR 0026](adr/0026-defer-populated-disaster-recovery-for-the-hobby-preview.md);
do not quietly turn this alpha into a new commercial durability promise.

## 11. Ticket-ready work packages, not newly filed issues

Before filing, reconcile against existing WorldStream issues to avoid duplicates.

| Work package | Scope and dependencies | Completion evidence |
| --- | --- | --- |
| Baseline and technical Lab | Phase 0; new docs/catalog distinction only after review, no retained identity changes. | Reproducible Heist technical suite and source/deployment record. |
| Bounded agent game-tool loop | After House policy extension; no kernel provider calls. | Tool sequence, cancellation, stale context and durable spending tests. |
| Flagship rules/evidence graph | After a paper-tested objective and two witness routes. | Valid/invalid/timeout/privacy fixtures and exact Component proof. |
| Flagship graphical client | Uses authorized Pack schema and shared client adapter. | Practice, live play, mobile/keyboard checks and no fixture leakage. |
| Hosted agent connection | After scoped admission/security decision. | Second-machine BYO, revocation, reconnect and no operator credential exposure. |
| Complete creator starter | Build alongside flagship, finalize after client/rules stabilize. | Fresh install and complete-game preview without editing runtime source. |
| Reviewed Distribution submission | Reuse artifact identities and Host approval; no arbitrary execution. | A new Pack and client added through metadata/artifacts, not central Pack branches. |
| Share/remix/result journey | Reviewed schema inputs and separate discovery/invitation/publication policies. | Friend invitation, private debrief and repeat-play evidence. |
| Closed-alpha verification | Exact artifacts plus approved small live-model spend. | Acceptance matrix, costs, known limitations and go/no-go report. |

## 12. Decisions to ratify before expanding implementation

| Recommendation | Existing constraint | Required treatment |
| --- | --- | --- |
| Keep Heist as PoC and create a distinct flagship | ADR 0014 already supports Heist demo/conformance status. | Document the product split; preserve existing identities. |
| Phaser first-party client | ADR 0017 permits independent rich clients. | No graphics kernel or universal renderer ABI. |
| Bounded House game tools and multi-step calls | ADR 0022 restricts tools/execution; ADR 0028 narrowly adds bounded plans. | Explicit follow-up policy/Runner contract, costs and failure tests; not an implicit renderer feature. |
| Hosted BYO connection and later remote MCP | Existing narrow browser/local paths are not blanket remote authority. | Scoped agent admission/authentication decision before exposing an endpoint. |
| Assisted game/scenario drafting | Current assistance only customizes a fixed starter. | Add reviewed deterministic authoring support; never auto-approve generated rules. |
| Creator previews through reviewed submissions | Current catalog is version-controlled and Host-approved. | Preserve that boundary for alpha; self-service execution requires a later decision. |
| Data-driven first-party client/catalog integration | Current first-party build lists are explicit. | Generate reviewed deployment inputs; do not dynamically execute untrusted modules. |
| Public custom clients, mutable publishing heads or leaderboards | Explicitly outside current ADR 0017/0019/0023 scope. | Defer or obtain separate security/data/competition decisions. |

### Not in this alpha

No kernel rewrite, second room engine, unrestricted code upload, hosted BYO
shells, crypto rewards, paid marketplace, 3D world editor, generic workflow
canvas, cross-Room shared memory, global skill rating, creator billing,
autoscaled fleet or universal natural-language game generator. Do not remove
accepted security checks or fabricate formal release evidence for convenience.

### The first action after approval

Begin with the smallest real-agent mission from phase 1 and the starter source
layout in parallel. Hold a short review of the bounded House tool contract
before coding that extension. Use plain temporary art, one real model route,
one named evidence object and a visible effect of redirection. Then test it
with a person who has not read our architecture.

If the person only waits or rubber-stamps the model, redesign the game before
adding content. If the creator must keep changing WorldStream, repair the
integration boundary before promising an ecosystem. Those two observations,
not the quantity of infrastructure, decide whether to continue expanding.

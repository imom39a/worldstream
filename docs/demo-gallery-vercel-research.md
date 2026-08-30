# WorldStream Technical Demo Catalog and Vercel Deployment Research

> - **Status:** Non-normative product and deployment research
> - **Current as of:** 2026-08-30
> - **Repository state reviewed:** local and remote default branch at `b0c1889c1e5f`, plus the existing uncommitted workspace
> - **Implementation status:** The selected catalog and Agent Heist fixture are implemented on `feature/demo-catalog-vercel`. The production prototype files were removed. This note does not claim that a Vercel deployment is complete.
>
> **Decision authority:** This note recommends a presentation and deployment
> direction. It does not change WorldStream's frozen requirements, security
> boundaries, protocol, runtime model, or release claims. Any adopted runtime or
> product change still needs the repository's normal specification and ADR
> process.

## Executive recommendation

Borrow Lightstreamer's **information architecture**, not its branding or
content:

1. make the first screen a filterable catalog of things a visitor can try;
2. give every card a working demo action and a deeper explanation action;
3. make every demo show one capability in less than a minute;
4. keep the live experience, technical explanation, and backend deployment
   independently evolvable; and
5. add entries through one typed catalog rather than hand-editing the landing
   page.

For this repository, the safest first release is one new static Vite application
at `web/demos`, deployed as one Vercel project, with route-based demos and a
data-driven catalog. The first card uses selected identity and Replay metadata
from `examples/heist/parity_fixture.json`. Its browser records are illustrative
summaries of the retained story. The browser does not execute the Activity Pack
Revision, Replay, or an authorization boundary. The same-site `How it works`
section is the first public explanation surface. A live WorldStream Negotiate
demo should be a second milestone, backed by a separately hosted, persistent
`worldstreamd`, not a prerequisite for launching the gallery.

This direction has four gates before implementation or deployment:

- **Git ownership:** Vercel Hobby cannot connect a private repository owned by a
  GitLab group or GitHub organization. A private personal-account repository
  avoids that categorical restriction; owner-authored commits are the safe
  Hobby path. The repository actually checked out here is the private personal
  **GitHub** repository `imom39a/worldstream`, not a GitLab repository.
- **Use classification:** Vercel Hobby is restricted to non-commercial personal
  use. A personal portfolio/demo can fit; a site advertising a product or paid
  capability requires Pro under Vercel's published fair-use definition.
- **Package manager:** this repository pins pnpm `11.19.0`, while Vercel's
  current supported-package-manager table lists pnpm 6 through 10. A preview
  build must prove that Corepack can supply the pinned version, or the project
  must make an explicit supported-toolchain decision before production.
- **Repository connection:** the personal Vercel Hobby scope
  `vinoth-3114s-projects` is accessible and currently has no projects. Its import
  screen asks to install the GitHub application, so the confirmed GitHub source
  is not connected yet. If a separate GitLab project is intended, its exact URL,
  ownership, and source-of-truth relationship must be identified first.

The prototype compared three layouts. The accepted direction combines the
Variant A catalog with the Variant C record inspector. The production site uses
that combination and does not retain the prototype switcher.

## Adopted implementation direction

The user confirmed these decisions on 2026-08-30:

- keep the source repository private and under the personal GitHub account;
- deploy from the local checkout, without a Git connection or CI/CD workflow;
- use one `web/demos` Vite application;
- publish Agent Heist first as a recorded browser fixture;
- keep WorldStream Negotiate in a planned state until a persistent authority exists;
- use Heroic Labs as a layout and visual reference;
- use ASD-STE100 as the writing basis; and
- maintain the adopted rules in `docs/demo-site-style.md`.

## Research method and limits

This review used primary sources only:

- the live [Lightstreamer Demo Gallery](https://demos.lightstreamer.com/), its
  live demo pages, Lightstreamer's official documentation, and repositories in
  the official Lightstreamer GitHub organization;
- Vercel's current official documentation and changelog; and
- this repository's checked-in source, configuration, and domain documentation.

The Vercel documentation used here reports update dates through 2026-08-25.
WebSocket support changed materially in June 2026, so older guidance saying that
Vercel Functions cannot serve WebSockets is obsolete. Read-only account and
repository checks were performed, but no Git provider was connected and no
project or deployment was created. Those checks found an empty personal Vercel
Hobby scope and no GitLab remote in the local Git configuration.

## What Lightstreamer's demo system actually does

### Information architecture

The gallery's value is not visual sophistication. Its strength is a consistent
path from discovery to a working demo and implementation detail.

| Layer | Observed behavior | Reusable lesson |
| --- | --- | --- |
| Entry | A direct promise: “Browse live examples and source code.” | State what visitors can do, not what the company is. |
| Primary filters | Product and component selects separate Broker/Connector examples and Client/Adapter examples. | Separate the thing being demonstrated from the integration surface. |
| Secondary filters | Domain categories (Collaboration, Finance, Gaming, Monitoring, News, Quickstart) and hierarchical platforms/frameworks narrow the catalog. | Let one demo have multiple useful facets; do not force a single folder-like hierarchy. |
| Result card | Title, client/platform label, screenshot, platform icons, category, optional badge, and independent **Live Demo** and **Details** actions. Some entries legitimately have Details but no live action. | Make availability explicit and never lead to a dead demo. |
| Live page | A standalone focused application, usually with a product banner, interactive state, short instructions, and a prominent GitHub ribbon/link. | Remove catalog chrome while the visitor is trying the experience, but retain an obvious path to explanation. |
| Details/source | The GitHub README normally includes a screenshot/live link, what the demo proves, client behavior, installation, server-side dependency, related examples, and compatibility notes. | Treat the explanation as a maintained product artifact, not an afterthought. |

The current gallery exposes these axes directly in its query-driven UI and card
grid: product, client/adapter component, application category, top-level
platform, and platform-specific technology. Lightstreamer described the same
four-way catalog model when it launched the browsable database: product,
component, application category, and platform, with a GitHub project and often a
live demo for each result. Sources:
[current gallery](https://demos.lightstreamer.com/),
[Lightstreamer 6.0 announcement](https://blog.lightstreamer.com/2015/01/lightstreamer-60-is-out.html).

The gallery preserves context by opening the live app and GitHub details as
separate targets. The live app is not forced into one universal shell: the
[Portfolio demo](https://demos.lightstreamer.com/PortfolioDemo/) is a compact
table/action/log application, while the
[React demo](https://demos.lightstreamer.com/ReactDemo/) includes a brief
framework explanation before the data UI. Both keep a direct source link.

### Demo taxonomy and live-data patterns

Lightstreamer demonstrates technical primitives through recognizable behavior,
then reuses the same backend feed to show different levels or clients.

| Official example | What the visitor experiences | Pattern worth adapting |
| --- | --- | --- |
| Basic Chat | Open multiple instances and see messages broadcast among them. The page warns that the sender's IP is displayed. | Explicit multi-window instruction plus a conspicuous privacy warning. |
| Basic Portfolio / Portfolio | Mutate a shared portfolio with buy/sell actions, watch the table change for every connected user, and see an order log. | Put user action, authoritative current state, and ordered consequences on one screen. |
| Basic Stock List to Stock List | Start with a ten-item minimal subscription example, then add dynamic lists, charts, per-view update frequencies, and resampling. | Offer a simple demo first and an advanced variant later; reuse one feed instead of building unrelated showcase data. |
| Room-Ball | Multiple users move avatars, type messages, and manipulate one shared object. | Make synchronization visible enough that two browser windows prove it immediately. |
| Race Telemetry | Show current state, an ordered lap history, and charts driven at different frequencies. | Present current projection and history together; high-frequency decoration should not obscure semantic events. |

Sources:
[Basic Chat client](https://github.com/Lightstreamer/Lightstreamer-example-Chat-client-javascript),
[Portfolio clients](https://github.com/Lightstreamer/Lightstreamer-example-Portfolio-client-javascript),
[Stock List clients](https://github.com/Lightstreamer/Lightstreamer-example-StockList-client-javascript),
[Room-Ball client](https://github.com/Lightstreamer/Lightstreamer-example-RoomBall-client-javascript),
[Race Telemetry client](https://github.com/Lightstreamer/Lightstreamer-example-RaceTelemetry-client-javascript).

The most important source-link pattern is the division of responsibility. A
client repository points to the adapter it needs, documents how to run against a
local or public server, and can be served by either Lightstreamer's embedded web
server or an external web server. Lightstreamer's architecture documentation
says the normal production shape separates synchronous/static content from the
real-time server: the browser downloads the page from a conventional web server
and receives asynchronous data from Lightstreamer. That is directly applicable
to WorldStream: Vercel can serve the public gallery while a separate authority
owns the live protocol and durable state. Sources:
[Hello World client deployment notes](https://github.com/Lightstreamer/Lightstreamer-example-HelloWorld-client-javascript),
[Lightstreamer General Concepts, architecture overview](https://www.lightstreamer.com/docs/ls-server/latest/General%20Concepts.pdf).

### What not to copy

- Do not copy Lightstreamer's logo, green styling, screenshots, text, or
  demo-domain content.
- Do not create technology/platform filters until WorldStream actually has
  meaningful client implementations on those platforms.
- Do not expose a “Source” action that sends public visitors to a private source
  control login page. Use **How it works**, **Read the protocol**, or **View build
  identity** until source publication is a deliberate decision.
- Do not reproduce globally writable shared demos without isolation, rate
  limits, reset behavior, and abuse/privacy handling.
- Do not equate constant visual motion with a WorldStream capability. The demo
  should make authoritative ordering, scoped views, reconnect, and replay
  visible.

## Language and visual design

The demo catalog is a technical interface. It is not a marketing site. Use
[ASD-STE100 Simplified Technical English, Issue 9](https://www.asd-ste100.org/assets/files/ASD-STE100_ISSUE9.pdf)
as the writing basis:

- Treat canonical WorldStream terms as approved project technical nouns. Use
  the exact same term for the same concept.
- Do not use slogans, metaphors, slang, or unsupported claims.
- Use one subject per sentence. Use a maximum of 25 words for descriptive text.
- Use a maximum of 20 words for instructions. Put one instruction in each
  sentence.
- Use direct button labels such as **Open preview**, **View capability
  details**, and **Close**.

Do not claim formal ASD-STE100 compliance until the text passes a controlled
dictionary check and a qualified human review. During development, label the
copy as an STE-based draft.

Use [Heroic Labs](https://heroiclabs.com/) as a visual reference, not as a
source of reusable assets. Adapt these general principles:

- Inter or a comparable plain sans-serif typeface;
- a purple primary surface, a light lavender section surface, slate text, and
  a turquoise status accent;
- a compact primary header followed by a centered technical introduction;
- a full-width, light section with equal-width cards and thin vertical or grid
  borders;
- a contrasting section band for status or scope data;
- centered section headings followed by structured technical detail;
- generous white space, simple cards, and clear status labels; and
- technical diagrams made for WorldStream rather than copied illustrations,
  icons, names, logos, or page content.

Keep Lightstreamer's catalog interaction inside that layout. Put search and
filters in a compact toolbar above the card grid. Do not copy the Heroic Labs
page order or use customer-logo, testimonial, pricing, or conversion sections.
The technical header must not contain a featured-demo or sales action. Show a
private source state as text, not as a link or button. Use normal interface text
sizes in cards and controls; do not reproduce small decorative website copy.

After plan approval, record the maintained rules in
`docs/demo-site-style.md`. That document should define tokens, typography,
layout, component states, diagram rules, accessibility, STE rules, prohibited
copy patterns, and a review checklist. New demo artifacts should cite and use
that document.

## A WorldStream-specific catalog

### Recommended facets

WorldStream should substitute product-relevant axes for Lightstreamer's broad
platform taxonomy.

| Facet | Initial values | Reason |
| --- | --- | --- |
| Activity Pack | Agent Heist, WorldStream Negotiate | Names the concrete shared situation. Counter can remain a tutorial rather than the product story. |
| Capability | Scoped views, Action Offers, Live observations, Catch-up/reset, Deterministic replay, Agent activation, Portable Packs, Evidence/interop | Lets a technical visitor find the exact claim they want to verify. |
| Experience | Interactive fixture, Guided replay, Live shared room | Makes the infrastructure requirement and degree of liveness honest. |
| Perspective | Participant, Spectator, Operator, Integrator, Pack Author | Maps demonstrations to the repository's domain roles without conflating them. |
| Availability | Available, Preview, Planned | Prevents design-only work from appearing shipped. |

Each entry should be a typed manifest record with a stable id, title, one-line
promise, Activity Pack, capability tags, experience mode, availability,
thumbnail, route, documentation URL, backend requirement, and optional build
identity. The catalog page should derive filters and cards from that record.
Adding a demo then becomes adding a route/module plus one catalog entry.

### Card anatomy

Every card should answer five questions without opening it:

1. What happens?
2. Which WorldStream capability does it prove?
3. Is it a fixture, guided replay, or live shared room?
4. Does it work now?
5. Where can I learn how it works?

The primary action should be **Try demo** only when the route is available. The
secondary action should be **How it works** and may point to the public manual.
Live status should be derived from a bounded health/readiness check, not a hard-
coded green badge.

### Demo-page anatomy

A common demo shell should contain:

- one sentence naming the capability being proved;
- an honest mode/status banner such as “Fixture — no network” or “Live shared
  room — reconnect enabled”;
- a 30–60 second guided checklist;
- the interactive stage;
- visible connection, room sequence, cursor, and replay state when relevant;
- a perspective switcher only where authorization semantics are the point;
- a “What this proves” panel and a small frontend/authority/data-flow diagram;
- reset/start-over behavior;
- links to public documentation and exact build/protocol identity; and
- an always-visible demo/privacy notice.

For shared demos, the Lightstreamer “open another browser window” move is worth
keeping. WorldStream should make the second window an authorized spectator or a
separately issued participant session, not copy a bearer credential into a URL.
The repository already treats credentials as outside URL and DOM state; the
public demo must preserve that boundary.

## What is already reusable in this repository

| Existing surface | Relevance to the gallery | Recommendation |
| --- | --- | --- |
| `web/manual` | Static React/Vite manual with a typed, filterable capability catalog and explicit Implemented/Reference/Design only/Deferred states. It already has GitHub and GitLab Pages publication paths. | Link it as **How it works** and reuse its status vocabulary and catalog concepts, not necessarily its entire visual shell. |
| `examples/heist/parity_fixture.json` | Records exact retained Agent Heist revision identity, final Room sequence, outcome, and read-only Replay verification. | Read selected safe metadata for the first browser demo. Label separate presentation records as illustrative. |
| `web/console` | Contains reusable console transport, fixture, and privacy patterns. Its current browser fixture is not an Agent Heist parity-view component. | Reuse patterns only. Do not claim that the demo page is the console fixture. |
| `web/console` Negotiate | Has fixture and live-session paths and demonstrates an application Activity Pack. | Add it after the catalog shell is stable; treat live mode as a separate backend milestone. |
| `web/studio` | Calls a loopback Supervisor and exposes operator workflows, daemon lifecycle, secrets, and room operations. | Do not publish it as a public demo. Build a bounded demo-specific surface instead. |
| `worldstreamd` | A stateful Rust authority with HTTP/WebSocket admission, SQLite/PostgreSQL storage, observation streams, and replay. It binds loopback by default and the product boundary is one live process. | Host it as a deliberately provisioned persistent service for live demos; do not disguise it as a static Vercel function migration. |
| `.gitlab-ci.yml` | Verifies and publishes only `web/manual`, with guidance to keep GitLab Pages private during the small-circle phase. | Leave it alone for the research/prototype stage. A public Vercel gallery is a separate deployment decision. |

The repository is already a pnpm workspace containing `web/console`,
`web/manual`, and `web/studio`. A sibling `web/demos` follows the established
layout and allows a Vercel Root Directory to point at one public application.
Using one project and one routed gallery is preferable to one Vercel project per
demo at this stage because the gallery is one coherent navigation and release
unit and Hobby allows only one concurrent build. If the confirmed GitHub
repository remains the source, Vercel's built-in “skip unaffected projects”
optimization is available; it is currently not available for GitLab-connected
monorepos.

### Verified repository and Vercel account state

Read-only inspection established the following current facts:

- `origin` is `https://github.com/imom39a/worldstream.git`.
- GitHub identifies `imom39a/worldstream` as a private repository owned by a
  **User**, not an organization; the authenticated `imom39a` account has admin
  access.
- `.gitlab-ci.yml` provides a GitLab Pages workflow, but a CI configuration file
  does not establish that a GitLab repository or remote exists.
- The accessible Vercel Hobby scope is `vinoth-3114s-projects`, and it currently
  has zero projects.
- The Vercel new-project screen currently offers to install the GitHub
  application, so GitHub has not yet been connected to that scope.

This resolves one ambiguity and exposes another. The confirmed private GitHub
repository is personal-account-owned, so the private-organization Hobby ban does
not apply to it. It should be eligible after the Vercel GitHub application and
the owner's login connection are configured. The requested “current GitLab”
cannot be evaluated as a source because no URL or remote has been identified.
Introducing a GitLab mirror only for Vercel would create an unnecessary second
source of truth unless there is a separate repository outside this checkout.

## Vercel feasibility for the confirmed GitHub and requested GitLab sources

### Git integration and private-repository rules

Vercel supports GitLab Free, Premium, Ultimate, Enterprise, and self-managed
GitLab through its documented flows. The native integration deploys each push,
creates a unique preview URL for merge requests, comments that URL on the merge
request, updates production from the configured production branch, and can roll
back when a deployed commit is reverted. Connecting a repository requires
Maintainer access; the GitLab OAuth integration requests read/write API scope so
it can clone and post deployment comments. Source:
[Vercel for GitLab](https://vercel.com/docs/git/vercel-for-gitlab).

Private-repository support depends on ownership and plan:

- A Hobby team **cannot** connect a private repository owned by a GitHub
  organization, GitLab group, or non-personal Bitbucket workspace.
- Vercel says the organization/group commit-author rule does not apply to
  collaborators on personal Git accounts. Its Hobby rule also says the commit
  author must be the owner of the destination Hobby team. The conservative setup
  for either a personal GitHub or personal GitLab repository is therefore to
  connect the owner's identity and deploy owner-authored commits.
- A Pro team permits private organization/group repositories, but commit
  authors must be team members under Vercel's collaboration rules.

Source: [Deploying Git repositories with Vercel](https://vercel.com/docs/git#deploying-private-git-repositories).

Therefore, “the repo can stay private on a personal Hobby account” is compatible
with the confirmed personal GitHub ownership, assuming the Vercel owner/login
connection and commit authorship line up. If an intended GitLab repository is in
a group, the clean choices are Pro or a deliberate source-ownership change; a
CI workaround should not be used to evade the plan's collaboration boundary.

### Monorepo and build configuration

Vercel lets a project select a Root Directory and supports multiple projects
from one repository. It currently permits 25 Vercel projects connected to one
Git repository on Hobby, but Hobby has only one concurrent deployment and 100
deployments per day. A single gallery project avoids multiplying push-triggered
builds and keeps one domain and rollback unit. Sources:
[Vercel monorepos](https://vercel.com/docs/monorepos),
[monorepo FAQ](https://vercel.com/docs/monorepos/monorepo-faq),
[Vercel limits](https://vercel.com/docs/limits).

The intended project settings are conceptually:

- Git repository: preferably the confirmed private personal GitHub repository;
  otherwise the exact personal GitLab repository, or a Pro project if it is
  group-owned;
- Production Branch: the repository's confirmed default/production branch;
- Root Directory: the new gallery app directory, proposed `web/demos`;
- Framework: Vite;
- Build: the app's workspace build command;
- Output: `dist`;
- Preview: every non-production branch/MR;
- Production: public only after prototype review and an explicit promotion.

Two toolchain facts need preview evidence. Vercel supports Node.js 24.x and uses
the latest patch in that major, while this repository pins exact Node
`24.18.1`. More importantly, Vercel currently advertises pnpm versions 6–10,
whereas this repository pins pnpm `11.19.0` in `packageManager`. Do not silently
rewrite the repository lockfile or toolchain. First run a private Preview build
using Corepack and record the exact build output; if it cannot honor pnpm 11,
make a separate, explicit toolchain decision. Sources:
[supported Node.js versions](https://vercel.com/docs/functions/runtimes/node-js/node-js-versions),
[Vercel package managers](https://vercel.com/docs/package-managers).

### Hobby constraints that matter

For a static first gallery, the important published Hobby limits are one
concurrent deployment, 100 deployments per day, 25 projects connected per Git
repository, 45 minutes of build time per deployment, and a typical monthly fair-
use guideline of 100 GB Fast Data Transfer. Static delivery should otherwise be
well inside the platform shape. Sources:
[Vercel limits](https://vercel.com/docs/limits),
[fair-use guidelines](https://vercel.com/docs/limits/fair-use-guidelines).

The legal/use constraint is more important than those quotas. Vercel says Hobby
is for non-commercial personal use and defines commercial usage to include a
deployment used for financial gain, including advertising a product or service.
If this gallery is a personal open-source portfolio with no commercial purpose,
Hobby may fit. If “demo of our capabilities” means business/product promotion,
the published rule points to Pro. This is a user/account-owner decision, not a
technical inference. Sources:
[Vercel Hobby plan](https://vercel.com/docs/plans/hobby),
[fair-use commercial usage](https://vercel.com/docs/limits/fair-use-guidelines#commercial-usage),
[Vercel Terms, Hobby Plan](https://vercel.com/legal/terms#4-hobby-plan).

### WebSockets, SSE, and the WorldStream authority

As of this review, Vercel Functions can natively serve WebSockets in public
beta. A connection is pinned to the accepting Function instance, but a later
connection may reach another instance. Connections close at the Function's
maximum duration; with Fluid compute, Hobby's default and maximum are 300
seconds. Durable rooms, presence, counters, and pub/sub must live outside
in-memory Function state, and clients must reconnect, resubscribe, and reload
state. WebSockets use normal Function and transfer usage. Sources:
[Vercel WebSockets](https://vercel.com/docs/functions/websockets),
[realtime publish/subscribe guidance](https://vercel.com/kb/guide/publish-and-subscribe-to-realtime-data-on-vercel),
[Function limits](https://vercel.com/docs/functions/limitations).

Vercel also supports streamed HTTP responses and SSE, but streamed requests are
still bounded by Function duration. SSE is useful for one-way resumable output;
it is not a reason to replace WorldStream's bidirectional protocol. Source:
[streaming with Vercel Functions](https://vercel.com/docs/functions/streaming-functions).

Native WebSockets do not make Vercel the best first home for the current
authority. The checked-in server is a long-running Rust process with durable
storage, startup readiness, one-process authority, and existing WebSocket
admission semantics. Vercel's WebSocket documentation currently gives supported
patterns for Node.js, Bun, and Python frameworks, while the Rust runtime is a
separate Function-oriented public beta and does not document Rust WebSocket
upgrade support. Replatforming the authority would introduce a second systems
project before the demo has validated its presentation.

Use this initial topology instead:

```text
Browser
  ├─ HTTPS ──> Vercel static gallery / demo UI
  └─ WSS/HTTPS ──> persistent WorldStream demo authority
                         └─ durable SQLite or PostgreSQL profile
```

That follows Lightstreamer's proven static-site/realtime-server separation and
preserves WorldStream's existing reconnect, cursor, reset, and replay model. The
live backend should expose only demo-specific sessions, exact allowed origins,
bounded capacity, rate limits, short-lived admission, reset/expiry, and no
operator or provider credentials.

### Keeping the repository private does not make the browser private

Vercel protects its source/build-log helper paths from third parties by default,
but every client asset shipped to the browser is public by definition. In a
Vite app, every `VITE_`-prefixed environment variable is statically bundled into
client code. Only public values such as the demo API origin may use that prefix;
tokens, database URLs, Git provider credentials, model keys, and authority
secrets may not. Sources:
[Vercel build/source protection](https://vercel.com/docs/builds/build-features),
[Vite environment variables](https://vite.dev/guide/env-and-mode.html),
[Vite on Vercel](https://vercel.com/docs/frameworks/frontend/vite).

The public demo should also omit the existing `VITE_SOURCE_URL` pattern when it
would point at a private repository. A private repo protects history and
server-side source distribution; it does not protect JavaScript sent to a
visitor.

## Proposed phased plan for confirmation

### Phase 0 — resolve go/no-go facts

1. Confirm that the private personal GitHub repository
   `imom39a/worldstream` is the intended Vercel source. If a different GitLab
   repository is intended, identify its exact URL, owner namespace, visibility,
   default branch, and relationship to GitHub.
2. Confirm whether the Vercel project is personal/non-commercial or requires
   Pro under the intended use.
3. If Hobby remains the target, connect the selected Git provider to the
   `vinoth-3114s-projects` scope and verify the Vercel owner/login connection and
   deploying commit author match. This is a state-changing setup step and should
   happen only after plan confirmation.
4. Keep one explicit source of truth. Do not add a GitLab mirror merely to deploy
   unless it is an intentional repository-management decision.
5. Prove Node/pnpm installation in a private preview build before production
   implementation depends on it.

### Phase 1 — throwaway UI prototype

Build several visually distinct gallery variants on one throwaway route, with a
floating variant switcher. Use real WorldStream names, statuses, and screenshots
or safe fixture thumbnails, but no live backend and no Vercel production
deployment. The prototype should settle:

- dense technical catalog versus story-led showcase;
- filter placement and mobile collapse behavior;
- card hierarchy and status language;
- whether demo pages open in-place or in a focused new tab; and
- the balance between “Try demo,” “How it works,” and build identity.

Record the selected direction, then discard the prototype from the production
branch as required by the prototype workflow.

### Phase 2 — static catalog and first demo

1. Add one `web/demos` workspace app and typed demo manifest.
2. Implement gallery, filters, demo-detail shell, honest availability states,
   metadata, accessibility, and responsive behavior.
3. Add one Agent Heist fixture/guided-replay demo using existing safe UI/data.
4. Link the public developer manual for details; do not link private source.
5. Add build/version identity and a clear “fixture — no network” notice.
6. Verify locally, then deploy only to a protected/private Preview for review.

### Phase 3 — production publication

1. Review the Preview for secrets, source URLs, generated assets, accessibility,
   narrow screens, social metadata, and accurate capability claims.
2. Confirm production visibility and custom-domain choice.
3. Promote the reviewed commit to public production.
4. Record the exact commit, Vercel project, production URL, and rollback point.

### Phase 4 — live WorldStream demo

1. Provision a persistent, demo-only WorldStream authority outside the static
   gallery deployment.
2. Add short-lived room/session creation, exact-origin CORS/WebSocket policy,
   abuse limits, expiry/reset, and bounded health/readiness.
3. Make two-window shared state, disconnect/catch-up, and deterministic replay
   the guided story.
4. Add WorldStream Negotiate only after the infrastructure story is reliable.
5. Keep fixture fallback available when the live backend is degraded.

### Phase 5 — grow one capability at a time

Add catalog entries only when each has a stable route, accurate explanation,
safe data, reset behavior, and availability monitor. Likely follow-ups are:

1. Scoped Views — compare public and participant Projections.
2. Catch-up and Reset — intentionally disconnect and resume from a Cursor.
3. Deterministic Replay — scrub one Room's canonical history.
4. Agent Attention — show a durable activation across fresh Invocations.
5. Portable Pack — build/inspect one exact Activity Pack Bundle without
   exposing operator authority.
6. Negotiate Evidence — complete a bounded agreement and verify its exported
   evidence offline.

## Confirmation decisions

Implementation should wait for explicit answers to these decisions:

1. Should Vercel use the confirmed private personal GitHub repository
   `imom39a/worldstream`, or is there a separate GitLab repository that has not
   been identified?
2. Is the public demo strictly a personal/non-commercial portfolio, or product
   and service promotion?
3. Is the initial scope accepted as **static gallery + Agent Heist fixture**, with
   live Negotiate deferred to a persistent backend milestone?
4. Should `web/demos` be one routed project, or is there a deliberate reason to
   pay the operational cost of separate Vercel projects per demo?
5. Which prototype direction should become production after the variants are
   reviewed?

## Source index

### Lightstreamer primary sources

- [Demo Gallery](https://demos.lightstreamer.com/)
- [Lightstreamer 6.0 gallery description](https://blog.lightstreamer.com/2015/01/lightstreamer-60-is-out.html)
- [General Concepts / architecture](https://www.lightstreamer.com/docs/ls-server/latest/General%20Concepts.pdf)
- [Hello World client](https://github.com/Lightstreamer/Lightstreamer-example-HelloWorld-client-javascript)
- [Basic Chat client](https://github.com/Lightstreamer/Lightstreamer-example-Chat-client-javascript)
- [Portfolio clients](https://github.com/Lightstreamer/Lightstreamer-example-Portfolio-client-javascript)
- [Stock List clients](https://github.com/Lightstreamer/Lightstreamer-example-StockList-client-javascript)
- [Room-Ball client](https://github.com/Lightstreamer/Lightstreamer-example-RoomBall-client-javascript)
- [Race Telemetry client](https://github.com/Lightstreamer/Lightstreamer-example-RaceTelemetry-client-javascript)

### Vercel and Vite primary sources

- [Deploying Git repositories](https://vercel.com/docs/git)
- [Vercel for GitLab](https://vercel.com/docs/git/vercel-for-gitlab)
- [Monorepos](https://vercel.com/docs/monorepos)
- [Monorepo FAQ](https://vercel.com/docs/monorepos/monorepo-faq)
- [Limits](https://vercel.com/docs/limits)
- [Hobby plan](https://vercel.com/docs/plans/hobby)
- [Fair-use guidelines](https://vercel.com/docs/limits/fair-use-guidelines)
- [Terms of Service](https://vercel.com/legal/terms)
- [Package managers](https://vercel.com/docs/package-managers)
- [Supported Node.js versions](https://vercel.com/docs/functions/runtimes/node-js/node-js-versions)
- [WebSockets](https://vercel.com/docs/functions/websockets)
- [Realtime publish/subscribe guidance](https://vercel.com/kb/guide/publish-and-subscribe-to-realtime-data-on-vercel)
- [Function limits](https://vercel.com/docs/functions/limitations)
- [Streaming functions](https://vercel.com/docs/functions/streaming-functions)
- [Rust runtime](https://vercel.com/docs/functions/runtimes/rust)
- [Build features and source protection](https://vercel.com/docs/builds/build-features)
- [Vite on Vercel](https://vercel.com/docs/frameworks/frontend/vite)
- [Vite environment variables](https://vite.dev/guide/env-and-mode.html)

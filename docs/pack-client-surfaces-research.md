# Pack-associated clients and participant surfaces

> - **Status:** Historical research; superseded by accepted ADR 0017 and the implemented Activity Client model
> - **Current as of:** 2026-09-01
> - **Repository baseline reviewed:** `dcde956`
>
> **Decision authority:** This note records the investigation that led to
> [ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md). The
> ADR and [current Activity Client guide](activity-clients.md) govern wherever
> this proposal's provisional names or roadmap differ from the implementation.

## Executive recommendation

The concern is valid. WorldStream currently has the correct security boundary
but the wrong presentation ownership.

The recommended model is:

> **An Activity Pack remains a headless deterministic rules artifact. A web
> experience is an optional, separately released Activity Client associated
> with exact Pack revision digests. Studio discovers and launches approved
> clients; it does not contain Pack-specific participant renderers.**

This produces four clear product surfaces:

1. **WorldStream Runtime** hosts Rooms and the exact deterministic Pack.
2. **Studio** operates the host, creates Rooms and Memberships, and brokers a
   scoped client launch.
3. **Activity Clients** present an activity. A browser app, CLI/TUI, Python
   process, mobile app, or agent-owned client is a protocol peer.
4. **WorldStream Inspector** is a first-party, Pack-neutral fallback for
   authorized projections, exact Action Offers, receipts, reconnect, and
   Replay. It is a workbench, not the intended experience for every Pack.

The Pack author may also be the client author, but those are separate release
roles. A Host Operator approves and selects a default client for an exact Pack
revision. A participant can use another compatible client with its own scoped
authority.

For Agent Heist, the immediate product move is **not** to make Studio render a
better Heist page. Extract a standalone first-party Agent Heist web client from
the good visual language at `web/demos`, give it recorded-fixture and live-room
adapters, and let Studio launch its live mode. The same renderer should power
the recorded demonstration and real participant experience; only the data
adapter and authority differ.

## Direct answers

### Should Studio provide participant views?

Studio should provide an **Open participant client** action, not a native
“Participant View.” That terminology matters: the thing opened is an
independent client, not a view owned by Studio. Studio may show Pack-neutral
launch status and bounded Host-Operator diagnostics. It must not impersonate a
Participant, contain a branch for each Pack, or receive participant-private
Projection data merely because the Host Operator opened Studio.

An embedded preview can be evaluated later. If implemented, it must run the
same independently released client in a different-origin sandbox with a
scoped participant or spectator session. It must never be a React component
dynamically imported into Studio's trusted JavaScript realm.

### Should every Pack have a UI?

No. Every Pack must be completely usable through the Room protocol and SDKs.
A Pack can have zero, one, or many associated Activity Clients:

- a generic Inspector only;
- a purpose-built web client;
- a public spectator screen plus a private participant client;
- a CLI/TUI;
- Python or TypeScript automation; or
- an agent client with no human UI.

The association is many-to-many. One client release can support several exact
Pack revisions after testing, and one Pack revision can have several clients.

### Who configures the UI?

The client developer publishes it. The Host Operator approves its exact bytes
and selects a deployment default. An Application Integrator or Host Operator
may choose among the approved compatible clients as non-canonical launch
metadata. The end participant
normally clicks **Open client**; they should not have to enter a URL or install
frontend dependencies.

The Room must not pin a mutable URL, and UI selection must not enter Genesis,
Authoritative Room State, Transitions, or Replay. Changing presentation must
not change what happened in the Room.

## What the current mismatch actually reveals

The two pages in question do different jobs:

- The [Agent Heist demo](../web/demos/src/AgentHeistPage.tsx) explicitly renders
  a recorded fixture, labels itself as having no network, and does not create a
  Room, submit an Action, or run Replay. The
  [demo workspace README](../web/demos/README.md) makes the same boundary
  explicit.
- The [Console entry point](../web/console/src/main.tsx) defaults to a
  Heist-shaped fixture when no retained handoff or direct bootstrap exists.
  A retained handoff instead enters the generic
  [HandedOffParticipant](../web/console/src/HandedOffParticipant.tsx), which
  special-cases Negotiate and otherwise displays Projection JSON plus generic
  Action forms.
- The Negotiate selection is a source-level Pack-ID branch in
  [negotiateHandoff.tsx](../web/console/src/negotiateHandoff.tsx).
- Studio's [participantViews.ts](../web/studio/src/participantViews.ts) already
  behaves mostly like a launcher: it requests an opaque handoff and opens the
  returned loopback URL.
- The direct live Heist path is more concerning than the visual mismatch.
  [App.tsx](../web/console/src/App.tsx) passes a complete fixture into
  `useLiveSession`; [liveSession.ts](../web/console/src/liveSession.ts) spreads
  that fixture and overlays only recognized fields from an authorized live
  Projection. A field absent from the live Projection can therefore retain a
  recorded fixture value. A live client must start from no activity data and
  install only an authorized Reset/Observation; fixture fallback data must be
  structurally impossible in live mode.
- Studio also has a Pack-specific leak in its operator shell:
  [roomOperatorView.ts](../web/studio/src/roomOperatorView.ts) accepts a
  Counter-shaped `{ counter: { value } }` result. Core/integrity diagnostics
  can remain native Studio features, but Pack-defined operator content must be
  a generic bounded projection or a separate associated client—not another
  Pack branch.

The visual discrepancy is therefore not one CSS defect. It exposes three
responsibilities currently combined in `web/console`: fixture showcase,
generic protocol inspector, and Pack-specific application UI. Adding a Heist
branch beside the Negotiate branch would make the next Pack more expensive and
would turn the Console into a centrally maintained renderer registry.

The repository's documented direction already resists that coupling:

- [ADR 0013](adr/0013-studio-companion-control-plane.md) defines Studio as a
  companion control plane, not the Participant Console.
- [Activity Pack Design](activity-packs.md#presentation-boundary) says a Pack
  publishes projections and semantic labels but no executable frontend code.
- [ADR 0014](adr/0014-installable-wasi-free-activity-pack-bundles.md) keeps the
  public Component at exactly five deterministic operations with zero imports.
- [UI Architecture](ui-architecture.md#external-clients) already names
  specialized external clients as the visualization extension path.

The proposal below completes those boundaries instead of changing the Room
Kernel.

## Research method

This review used primary sources only: official specifications, official
documentation, upstream repositories, and this repository's source and accepted
ADRs. The comparison tests one hypothesis:

> Can WorldStream keep `.wspack` deterministic and headless while making a
> Pack-specific experience discoverable, safe to launch, independently
> upgradeable, and optional?

The answer is yes. No single reviewed system should be copied whole, but their
separation, manifest, compatibility, artifact-association, and sandbox
patterns compose well.

## Relevant systems and lessons

| System | Primary-source behavior | Lesson for WorldStream |
| --- | --- | --- |
| Lightstreamer | Its server handles real-time connections while client libraries integrate browser, mobile, and desktop applications. Its normal production architecture serves ordinary web content from a conventional web server and asynchronous data from Lightstreamer. Adapters can also run separately from the kernel. Sources: [General Concepts, architecture and clients](https://lightstreamer.com/docs/ls-server/latest/General%20Concepts.pdf), [official Hello World client](https://github.com/Lightstreamer/Lightstreamer-example-HelloWorld-client-javascript). | The streaming runtime does not need to own application presentation. A standalone Heist client and Python client can be equal consumers of one live protocol. |
| MCP Apps | A tool can associate a `ui://` resource with an interactive view. Web hosts render views in a sandbox, enforce resource-declared CSP/permissions, and mediate host/view communication through a protocol. The versioned specification requires a different-origin sandbox proxy and restrictive defaults. Sources: [MCP Apps overview](https://modelcontextprotocol.io/extensions/apps/overview), [MCP Apps 2026-01-26 specification](https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx#sandbox-proxy). | Borrow optional discovery, capability negotiation, sandboxing, and a narrow message bridge. Do not make the deterministic Pack itself an MCP UI resource or give UI code Pack authority. |
| VS Code | Extensions declare host compatibility in a manifest. Its webview guidance says to enable only required capabilities, restrict local resources, and start CSP from `default-src 'none'`. Sources: [extension manifest](https://code.visualstudio.com/api/references/extension-manifest), [webview security](https://code.visualstudio.com/api/extension-guides/webview#security). | A client artifact needs explicit compatibility and least-capability metadata. A host should enforce stronger policy than the client requests. |
| Grafana | A frontend plugin has a metadata manifest, frontend entry point, and declared Grafana dependency. Grafana requires signed plugins by default; its frontend sandbox adds iframe isolation for selected plugins, although that sandbox is currently opt-in. Sources: [plugin anatomy](https://grafana.com/developers/plugin-tools/key-concepts/anatomy-of-a-plugin/), [`plugin.json`](https://grafana.com/developers/plugin-tools/reference/plugin-json), [plugin signing](https://grafana.com/developers/plugin-tools/publish-a-plugin/sign-a-plugin), [frontend sandbox](https://grafana.com/docs/grafana/latest/administration/plugin-management/plugin-frontend-sandbox/). | Keep client metadata, compatibility, provenance, and executable bytes together, but require isolation for any embedded third-party client instead of relying on an optional setting. |
| Backstage | Frontend plugins normally ship as separate packages installed into an app. Extensions have stable IDs, typed inputs/outputs, attachment points, configuration, and conditions. Sources: [frontend plugins](https://backstage.io/docs/frontend-system/architecture/plugins/), [frontend extensions](https://backstage.io/docs/frontend-system/architecture/extensions/), [plugin installation](https://backstage.io/docs/frontend-system/building-apps/installing-plugins/). | Stable extension contracts are useful, but build-time package composition is suitable only for trusted first-party Studio features. It is too coupled for arbitrary Pack clients loaded at runtime. |
| OCI artifacts | An OCI manifest can name another manifest as its `subject`; the Distribution Specification exposes referrers associated with a subject digest. Sources: [OCI Image Manifest `subject`](https://github.com/opencontainers/image-spec/blob/main/manifest.md#image-manifest-property-descriptions), [OCI Distribution referrers](https://github.com/opencontainers/distribution-spec/blob/main/spec.md#listing-referrers). | Model a client release as a separate digest-addressed artifact referring to one or more exact Pack revisions. WorldStream can borrow the relationship without adopting a network OCI registry in the first release. |

### Derivation

The common pattern is not “plugins render inside the admin application.” It is:

1. keep the authority/runtime interface independent of presentation;
2. identify extension bytes and compatibility explicitly;
3. install or approve executable presentation separately;
4. give it only a narrow client capability; and
5. preserve a fallback when the specialized extension is absent.

That is a closer fit for WorldStream than a View ABI inside `ActivityPackV1`.

## Proposed domain model

The following names are provisional and should be reconciled through the
domain-modeling process before becoming normative.

| Proposed term | Meaning | Not this |
| --- | --- | --- |
| **Activity Client** | Any independently executing application that attaches to a Room Membership and consumes the public protocol. | Pack executor, Runner, Studio plugin |
| **Web Client Surface** | A static browser Activity Client release with a declared entry point and security policy. | React component imported by Studio |
| **Client Surface Bundle** | Proposed offline, content-addressed package containing one web client release and its manifest. A provisional `.wsclient` suffix can make it distinct from `.wspack`. | Activity Pack Bundle |
| **Surface Binding** | Host-approved association from an exact Pack revision, Projection schema, and Access Mode to an exact Client Surface Bundle digest. | Canonical Room state or mutable URL inside a Pack |
| **WorldStream Inspector** | First-party Pack-neutral diagnostic client and graceful fallback. | Intended UI generator for every activity |
| **Client Host** | Non-authoritative static asset and launch service. It may initially be a bounded Supervisor module. | `worldstreamd` or a second Room authority |

“Pack-owned UI” is convenient conversational shorthand, but
**Pack-associated client** is the accurate model. It permits independent client
authors and several experiences for one ruleset.

## Proposed architecture

~~~mermaid
flowchart LR
    P[".wspack<br/>deterministic rules"] --> D["worldstreamd<br/>Room authority"]
    D --> R["Room pins exact<br/>Pack revision"]

    C[".wsclient<br/>optional web client"] --> H["Client Host<br/>approved static bytes"]
    C -. "declares exact compatibility" .-> P
    S["Studio<br/>operator + launcher"] --> H
    S --> D
    H --> B["Browser Activity Client"]

    B -->|"authorized projection / action"| D
    CLI["CLI or TUI"] -->|"same client contract"| D
    PY["Python application"] -->|"same client contract"| D
    A["Agent participant client"] -->|"same client contract"| D
~~~

Two constraints matter:

- `worldstreamd` remains correct and usable when the Client Host, Studio, and
  every web bundle are absent.
- A Client Surface Bundle is presentation software. It never runs in the Pack
  Wasmtime instance, affects a Transition, receives raw Activity State, or
  becomes part of Replay.

### Client Surface Manifest v1 sketch

The exact schema should be designed in an ADR. This sketch shows the minimum
information needed to test the architecture:

~~~json
{
  "schema": "worldstream/client-surface-manifest/v1",
  "surface_id": "worldstream.agent-heist.web",
  "display_name": "Agent Heist",
  "explanatory_version": "0.1.0",
  "entrypoint": {
    "kind": "static_web",
    "path": "index.html"
  },
  "compatible_pack_revisions": [
    {
      "pack_id": "worldstream.agent-heist",
      "revision_digest": "blake3:<exact-pack-revision>",
      "projection_schema_digests": ["blake3:<exact-schema>"],
      "action_schema_digests": ["blake3:<exact-schema>"]
    }
  ],
  "client_contracts": ["worldstream/room-client/v1"],
  "access_modes": ["participant", "spectator"],
  "operations": ["observe", "ack", "act", "replay"],
  "launch_modes": ["external_tab"],
  "requested_policy": {
    "network": "session_bridge_only",
    "browser_permissions": [],
    "nested_frames": []
  }
}
~~~

`access_modes` uses the WorldStream Core values, not a UI-defined audience
vocabulary. A client that supports a participant Role still binds the exact
participant Projection schema; it does not gain other Roles or Access Modes.

The bundle digest must be computed over the complete canonical archive and
stored outside its own manifest, avoiding a self-hash cycle. The explanatory
version is for people. Launch compatibility uses exact digests, never a name or
SemVer fallback.

The manifest is not permission. The installed binding is admitted only after
the Host Operator approves the exact physical bundle digest. A signature may
prove publisher identity, but it must not replace exact-byte verification and
local approval. This is consistent with WorldStream's current Pack trust model.
Sigstore's blob workflow is one available provenance mechanism: its signed
bundle carries signature, certificate, and transparency-log verification
material, and verification can constrain the expected signing identity and
issuer. Sources: [Sigstore blob signing](https://docs.sigstore.dev/cosign/signing/signing_with_blobs/),
[Sigstore verification](https://docs.sigstore.dev/cosign/verifying/verify/).

### Where discovery belongs

Do not add an executable UI or mutable URL to `descriptor.json`, `.wspack`, or
Room configuration. Instead, maintain a local Client Surface Registry:

~~~text
(pack_revision_digest, projection_schema_digest, access_mode, surface_id)
    -> approved_surface_bundle_digest + local entrypoint

approved_surface_bundle_digest
    -> host-configured exact serving origin
~~~

The serving origin is a deployment binding, not artifact identity. The same
approved bytes can be hosted at different exact origins without rebuilding the
client, while the Host still rejects an origin not configured for that digest.

The registry is deployment state owned by the Client Host/Supervisor. Studio
can read its bounded metadata and offer these actions:

- **Open Agent Heist** when an exact compatible default is installed;
- **Choose another approved client** when several match;
- **Open generic Inspector** when none match; and
- **Use SDK/CLI** with documentation for non-browser clients.

No Pack projection, action payload, Room text, or participant can supply a
launch URL. A network catalog and automatic installation remain deferred.

The OCI subject/referrers pattern is useful if a registry is added later: the
Pack revision is the subject, while client artifacts refer to it. The first
implementation only needs the same content-addressed relation in a local
catalog.

## Launch and authority lifecycle

A safe first-party local launch can reuse the good part of the current handoff:

1. Studio asks the Supervisor to create participant access for one selected
   seat and exact approved surface.
2. The Supervisor verifies that the surface binding matches the Room's pinned
   Pack revision, exact Projection schema digest, Access Mode, and configured
   serving origin.
3. It returns a launch URL built from the installed local entry point and a
   one-use opaque handoff in the fragment.
4. Studio opens that URL in a new tab with `noopener,noreferrer`.
5. The Activity Client scrubs the fragment immediately, redeems the handoff,
   and receives only a retained HttpOnly participant session.
6. The client uses a shared browser Application SDK for observation reset,
   catch-up, acknowledgement, exact-head Action submission, reconnect,
   receipts, and authorized Replay.

The surface receives neither Host Operator authority nor Runner authority. Its
declared operations are an upper bound; the actual Membership session remains
the authority.

For a future remote or third-party browser client, do not generalize this into
`?redirect=<arbitrary-url>` or place a bearer in a URL. OAuth Security BCP
requires exact matching for pre-registered redirects, prohibits open
redirectors, and requires PKCE for public clients; it also recommends the
`S256` challenge. Use an authorization-code-style, transaction-bound launch
with exact registered origins and PKCE if WorldStream reaches that stage.
Source: [RFC 9700 sections 2.1 and 4.1](https://www.rfc-editor.org/rfc/rfc9700.html#name-protecting-redirect-based-flo).

### Browser, CLI, Python, and agents are peers

The launch catalog must not imply that browser UI is the protocol. The
[Python SDK](../sdk/python/README.md) already opens a Room Membership, installs
observations, acknowledges durable processing, and handles safe Action retry.
The web client needs the same reusable SDK boundary extracted from
`web/console`; Pack UI code should receive a client/session interface rather
than reimplementing WebSocket state.

CLI/TUI, Python, mobile, and agent clients should not be executable entries in
the web bundle. The catalog may show signed documentation or package metadata,
but Studio must never install dependencies or execute an author-supplied
command. Those clients obtain their own scoped credentials through an explicit
operator flow.

## Security requirements

### Standalone client

For the first release, open an installed first-party client in a separate tab
and separate origin. Serve the exact approved static bytes locally with an
enforced response-header CSP. A reasonable default is:

~~~text
default-src 'none';
script-src 'self';
style-src 'self';
img-src 'self' data:;
connect-src <exact-session-bridge-origin>;
object-src 'none';
base-uri 'none';
form-action 'none';
frame-ancestors 'none';
~~~

The W3C CSP specification defines `connect-src` as the control for fetch,
XHR, EventSource, and WebSocket destinations; `frame-ancestors` controls who
may embed a resource, and it does not inherit `default-src`. These policies
therefore need explicit response-header enforcement. Source:
[Content Security Policy Level 3](https://www.w3.org/TR/CSP3/).

An exact digest proves which bytes were installed, not that the UI is honest.
Studio should display publisher/provenance status and the operations requested
before approval. Browser permissions, external network origins, nested frames,
popups, downloads, camera, microphone, geolocation, and clipboard access are
denied unless a later explicit policy supports them.

### Embedded preview, if added later

An embedded third-party client must use a different-origin sandbox, not a
dynamic import. The host should mediate a small versioned message protocol,
validate message source, origin, schema, size, and operation, and never forward
unknown methods. The HTML Standard warns that a same-origin iframe with both
`allow-scripts` and `allow-same-origin` can remove its sandbox; it recommends a
dedicated origin for potentially hostile content. Source:
[WHATWG iframe sandbox guidance](https://html.spec.whatwg.org/dev/iframe-embed-object.html#attr-iframe-sandbox).

The MCP Apps sandbox proxy is a useful concrete pattern: separate origins,
restrictive CSP defaults, explicit permission metadata, and protocol-mediated
communication. WorldStream should copy the shape, not the MCP tool semantics.

### Invariants

1. Client bytes never enter the Pack Component or canonical Room history.
2. UI absence or failure never prevents SDK/CLI use of the Room.
3. A client sees only the Projection and Action Offers authorized for its
   Membership.
4. Studio never receives participant-private data to make client launch work.
5. A surface mismatch fails closed to the Inspector; it never chooses by Pack
   name or explanatory version alone.
6. The host serves only exact approved surface bytes and records the exact
   digest used for a launch in operational audit.
7. Updating a client does not reinterpret or mutate retained Room lineage.
8. No client-supplied URL becomes a redirect or asset origin.
9. Embedded code never shares Studio's trusted origin or JavaScript realm.
10. The generic Inspector has no Pack-ID branches and invents no legality
    rules; it renders only exact authorized data and schemas.

## Agent Heist migration proposal

### Desired result

The recorded demo and live participant app should look recognizably identical
because they use the same Heist presentation components. They must remain
honest about different execution modes:

- **Recorded mode:** checked-in parity records, no network, no real authority,
  no Action submission.
- **Live spectator mode:** a spectator Membership and public authorized stream.
- **Live participant mode:** one participant Membership, private authorized
  Projection, exact Action Offers, Actions, reconnect, and Replay.

A live session exposes exactly its authenticated Access Mode and authorized
Projection. The recorded demo may switch illustrative lenses, but a live
participant must not switch to Public or Operator by changing a browser tab.
Those are separate authorized Membership sessions.

Recorded and live models should be different types. A live view model starts
empty/unavailable, atomically installs an authorized Projection Reset, and then
applies authorized Observations. It has no `fixture` field and no default value
path. The recorded adapter cannot be imported as a live adapter or satisfy its
constructor.

The current demo's record inspector can remain as a technical Replay/visibility
experience. Add the actual game/participant panels described in
[UI Architecture](ui-architecture.md#agent-heist-view) as shared Heist client
components, not Studio components.

### Repository shape

One plausible target is:

~~~text
sdk/typescript-client/                 # Pack-neutral Room client/session SDK
clients/agent-heist-web/              # first-party Heist Activity Client
  src/domain/                          # Heist projection -> view model
  src/components/                      # Pack-specific presentation
  src/adapters/recorded.ts             # checked-in fixture adapter
  src/adapters/live.ts                 # WorldStream client SDK adapter
clients/negotiate-web/                 # later extraction from web/console
web/inspector/                         # renamed Pack-neutral fallback
web/studio/                            # operator portal + client launcher
web/demos/                             # catalog; mounts recorded client mode
~~~

The precise directory names are less important than dependency direction:

~~~text
Heist UI -> Heist view model -> generic client session interface
Studio   -> surface registry + launch API
Runtime  -> no dependency on any of the above
~~~

### What to remove from the current Console

After the standalone clients work:

- move Heist-specific fixture/components out of `web/console`;
- move Negotiate-specific rendering out of `web/console`;
- remove Pack-ID renderer branches;
- retain the handed-off Pack-neutral observation/action/replay workbench as the
  Inspector; and
- preserve the one-time handoff/session proxy as shared client infrastructure.

## Options considered

| Option | Assessment |
| --- | --- |
| Add better native Heist and Negotiate views to Studio | Reject. Fast for two Packs, but makes Studio a participant client, central renderer registry, and privacy risk. |
| Put HTML/JavaScript inside `.wspack` | Reject. It violates the accepted deterministic Component profile and couples semantic retention to mutable presentation software. |
| Generate every UI from schemas | Keep only as Inspector fallback. Schemas can produce forms and tables, but they cannot express the useful Heist map, negotiation comparison, or domain explanation without creating a second UI language. |
| Generate executable UI with an LLM at runtime | Reject. It makes security review, repeatability, accessibility, compatibility, and support worse. Prompt assistance may author an ordinary client project before build and review. |
| Compile trusted UI plugins into Studio | Allow only for Studio's own first-party operator features. Backstage demonstrates this model, but it does not solve independently released Pack experiences. |
| Load arbitrary remote UI in a Studio iframe | Defer. Sandboxing can reduce access, but remote mutability, origin policy, consent, and launch authorization make it the wrong first milestone. |
| Separate, exact-digest Activity Clients plus generic fallback | **Recommend.** It preserves the Kernel, supports an ecosystem, and gives Pack authors full presentation freedom without forcing a UI on non-browser clients. |

## Implementation roadmap

### Phase 0 — accept the boundary

Write one ADR that establishes:

- Activity Clients as protocol peers;
- Client Surface Bundles as separate from Activity Pack Bundles;
- exact many-to-many Pack revision compatibility;
- Studio as catalog/launcher only;
- Inspector as Pack-neutral fallback; and
- no network registry or arbitrary third-party code in the initial release.

Update `CONTEXT.md`, the glossary, FR-9, Studio, UI architecture, Activity Pack
design, and the roadmap only after that ADR is accepted.

### Phase 1 — extract the reusable browser client contract

- Move transport, retained handoff, observation, acknowledgement, reconnect,
  exact Action, receipt, and Replay logic into a Pack-neutral TypeScript SDK.
- Make the current generic handed-off view consume only that SDK.
- Rename the generic product surface to WorldStream Inspector.
- Add conformance fixtures shared with the Python SDK.

This phase changes no Runtime or Pack ABI.

### Phase 2 — ship the standalone Agent Heist client

- Extract a Heist view model and components.
- Support recorded and live adapters with disjoint state types and no shared
  fallback data.
- Implement public/spectator and participant presentation from authorized
  projections.
- Make the demo catalog mount recorded mode.
- Make local development launch live mode directly before adding discovery.

Acceptance requires two browser windows to make shared ordering visible, a
private clue to remain absent from public DOM, reconnect to preserve cursor
semantics, stale controls to disable during catch-up, and Replay to remain
read-only. Removing any field from a live Projection must remove it from the
DOM rather than reveal a fixture default, and live mode must expose no
fixture-lens switcher.

### Phase 3 — add local surface bundles and launch discovery

- Freeze `client-surface-manifest/v1` and an offline bundle format.
- Add inspect, approve, install, inventory, select-default, and remove commands
  for client bytes in a namespace distinct from Pack commands.
- Build the Supervisor's read-only catalog and exact launch endpoint.
- Fail closed to Inspector on missing, unapproved, or incompatible surfaces.
- Serve client bytes on an isolated local origin with host-enforced CSP.

Do not add hot network discovery, automatic install, or a marketplace.

### Phase 4 — extract the Negotiate client

- Move curated negotiation rendering and evidence controls out of Console.
- Bind its exact client release to the exact public Negotiate Pack revision.
- Prove that Studio launches Agent Heist, Negotiate, and Inspector through one
  identical discovery path with no Pack-ID branch.

### Phase 5 — evaluate third-party embedding

Only after outside Pack authors need it, specify a sandbox bridge and permission
contract using MCP Apps, CSP, and browser sandbox guidance as inputs. Remote
clients, registries, signing policy, and marketplace UX are later independent
decisions.

## Recommended first ticket set

1. **ADR: separate Activity Clients from Activity Pack execution.**
2. **TypeScript Application SDK extraction from `web/console`.**
3. **WorldStream Inspector rename, “participant client” terminology, and
   Pack-branch removal plan.**
4. **Agent Heist view-model boundary with type-separated recorded/live
   adapters and an empty authorized live baseline.**
5. **Standalone Agent Heist live participant client.**
6. **Client Surface Manifest v1 schema and malicious-corpus tests.**
7. **Local exact-digest Client Surface Registry and Studio launcher.**
8. **Negotiate standalone client extraction.**
9. **Cross-client conformance: browser, Inspector, and Python observe the same
   authorized sequence and submit the same canonical Action shape.**

## Decision

Adopt the architecture direction, but do not begin with the bundle registry.
The fastest proof is:

> Extract one reusable browser client session, build Agent Heist as a standalone
> client with recorded and live adapters, and make Studio open it through the
> existing one-use handoff.

If that produces a good experience without changing the Kernel or Pack ABI,
then freeze the generic Client Surface Manifest and installation lifecycle. If
it does not, the project has learned that lesson before creating another public
plugin contract.

---
status: accepted
date: 2026-09-01
---

# Separate Activity Clients from Packs and Studio

[ADR 0018](0018-cli-first-operator-surface.md) replaces the permanent Studio
operator-web-product commitment with a gated CLI-first transition. The
independent Activity Client, Pack, authorization, and binding contracts in
this decision remain in force. Studio is not removed until its replacement
is verified.

## Context

WorldStream already keeps `worldstreamd` authoritative and Activity Pack
execution deterministic, but the first web implementation combined three
different products in one Participant Console: recorded Agent Heist fixture
inspection, Pack-neutral protocol inspection, and Pack-specific participant
rendering. Studio then launched that one Console regardless of the Room's exact
Activity Pack Revision.

That packaging makes a polished Pack experience look like a generic JSON
debugger, encourages Pack-ID branches in one centrally maintained frontend,
and creates pressure either to put executable presentation code in `.wspack`
or to make Studio a Participant client. Both choices weaken accepted Pack and
Studio boundaries.

## Decision

An **Activity Client** is an independently executing application that uses a
scoped WorldStream client contract for one or more exact Activity Pack
Revisions. Browser applications, CLI/TUI programs, mobile applications,
service applications, and agent-owned clients are protocol peers. Clients are
**protocol-thin and experience-rich**: they may contain Pack-specific
presentation, interaction, and disposable local UI state, but they do not own
Room rules, infer Action legality, reconstruct hidden truth, or mutate
Authoritative Room State. An Activity Pack may have zero, one, or many Activity
Clients, and an Activity Client may support more than one exact Pack revision
after explicit compatibility testing.

An Activity Pack Bundle remains headless. It carries deterministic rules,
schemas, codecs, immutable static material, and conformance evidence. Static
material is never served or executed by WorldStream as Activity Client code;
its filename or media type is not the security boundary. The Bundle carries no
mutable launch URL or frontend authority. Client selection and deployment are
operational integration state. They never enter Genesis, Authoritative Room
State, Transitions, canonical hashes, or Replay.

An **Activity Client Release** is one immutable, content-addressed published
build. Its release manifest identifies exact runnable artifacts, Client
Surfaces, supported client-contract version, and linked conformance evidence.
One Release may expose several Client Surfaces. Products needing independent
cadence, evidence, or trust publish separate Releases even when one development
server happens to serve their paths. A **Client Deployment** is one Host
Operator-approved operational availability of an Activity Client Release at an
exact independently executing launch target. Release identity, deployment
readiness, and compatibility selection remain separate facts.

Compatibility declarations, conformance evidence, SBOMs, signatures, and
attestations are separately identified artifacts linked to the Release. They do
not silently alter its payload identity. Host approval retains the exact
Release identity and every evidence identity required by Host policy. A
publisher compatibility claim never replaces a Client Binding.

The Activity Client Release manifest contains only bounded publisher claims:
its stable client ID, exact Release digest, supported client-contract version,
exact artifact IDs/media types/digests, Client Surface IDs/kinds/artifact
references/entrypoints/capabilities, and linked conformance evidence. Exact Pack, Access
Mode, and Role compatibility belongs to an Activity Distribution and the
Host's Client Bindings rather than being a second authority inside the Release.
Additional SBOM, signature, and attestation identities may be linked by later
distribution policy. A Release contains no live deployment origin, credential,
Deployment Trust Level, Host approval, or default Client Binding.

A Client Deployment has an explicit **Deployment Trust Level**. A verified
deployment has evidence that the deployment facility started the exact
Activity Client Release digest. An externally trusted deployment is an exact
target that the Host Operator approved without proof of its running bytes. An
origin, readiness response, or self-declared version does not by itself prove
an Activity Client Release. Host policy may reject externally trusted
deployments, and Studio may display the generic trust classification without
interpreting client implementation details.

An **Activity Distribution** is an Application Integrator-owned, versioned,
machine-readable manifest. It references one or more exact Activity Pack
Bundles, separately identified Activity Client Releases, optional Runner
integrations, documentation, and deployment templates. A template may propose
ports, bounded resource capabilities, secret names, and other non-secret
defaults, but it contains no capability approval, secret value, approved
origin, live credential, handoff secret, approved Client Deployment, or Client
Binding Store state. A release may deliver the referenced artifacts together,
but the manifest is not an executable extension of `.wspack` and never merges
the Pack, client, Runner, or Host trust domains. The manifest is operational
integration material and never enters Genesis, Authoritative Room State,
canonical hashes, or Replay.

An Activity Distribution is a portable proposal, not installed Host truth.
Each WorldStream installation owns a **Client Binding Store** containing
Host Operator-approved Activity Client Releases, Client Deployments, and
**Client Bindings**. A Client Binding associates an exact Activity Pack
Revision and client-contract version, one Access Mode, and an explicit
applicable participant Role set with one Host-brokered Client Surface of an
approved Client Deployment. Exact Pack ID and version remain diagnostic labels;
the semantic Activity Pack Revision digest is compatibility authority. A
participant binding may support all declared Roles or only an explicit subset.
Supported Projection schema identities and digests are compatibility evidence
rather than parallel selection authority. Direct CLI, service, SDK-built, and
agent-owned clients authenticate independently and do not require a Client
Binding or Studio selection. These records are durable Supervisor operational
state outside the Room Kernel, Room state, and Canonical History. Pack
authorship, artifact publication, and Application Integration grant no implicit
approval: the Host Operator separately approves each Pack Bundle, Activity
Client Release, Client Deployment, and Client Binding used by that Host.

The open long-term artifact transport for self-hosted WorldStream releases is
the OCI Distribution protocol. Activity Pack Bundles, Activity Client
Releases, Activity Distributions, conformance evidence, signatures, and
attestations may use distinct WorldStream media types while retaining their
separate identities. Exact manifest digests identify artifacts; mutable tags
are discovery labels only. Existing Artifact Registries may provide transport,
but publication or retrieval grants no Host approval. Language ecosystem
registries such as npm, PyPI, and crates.io distribute Client SDKs and build
libraries; they are not Activity Client selection or approval authorities.

OCI is a transport envelope, not a replacement identity. The semantic Activity
Pack Revision digest remains Room compatibility authority, and the exact
physical `.wspack` BLAKE3 remains Activity Pack Bundle approval identity. An OCI
manifest digest identifies the exact transported OCI object. For an OCI-native
Activity Client Release, that manifest digest is also its Release identity; for
an OCI-wrapped `.wspack`, it does not replace either WorldStream Pack digest.

For each browser handoff, the Host performs **Client Selection** from the
Membership's current authoritative Activity Pack Revision, Access Mode, and
Role rather than from a stale Studio draft or client claim. It filters approved
Client Bindings by client-contract version, Deployment Trust Level policy, and
readiness. An explicit approved Binding request wins; otherwise one
Host-selected default wins; otherwise one remaining candidate wins. Multiple
remaining candidates produce `selection_required`, and no remaining candidate
offers the configured Inspector fallback. Selection never uses mutable tags,
semantic-version ranges, registry order, or client self-selection.

Studio may present a Pack-neutral chooser after `selection_required`. It may
show only generic approved metadata such as Activity Client Release name and
identity, Client Surface label, Deployment Trust Level, and readiness. One
explicit choice applies to one handoff. A Host-selected default is durable
Client Binding Store configuration. Neither enters Core Room State, Activity
State, Membership, Canonical History, or Replay.

Activity Client Release, Client Deployment, and Client Binding identities are
immutable. A replacement creates new records and requires approval; no tag,
publisher update, or changed bytes behind a target silently upgrades one.
Changing a Host default is explicit operational state. Disabling a Client
Binding blocks new handoffs without rewriting old records. Revoking a Client
Deployment blocks new handoffs and invalidates its active brokered browser
sessions.

Client Selection is re-evaluated when a browser client's Membership Access
Mode or Role changes. The session continues when its Client Binding remains
applicable. Otherwise the Host invalidates that browser client session and
requires a new handoff; it does not silently redirect the browser. SDK-built
and agent-owned Activity Clients that connect directly remain responsible for
handling each newly authorized Projection.

Studio may import an exact Activity Distribution reference and manage its
proposed artifacts through generic Host Operator operations. It is not a public
Client Catalog or marketplace and does not provide client discovery, ratings,
search, publisher promotion, or automatic trust. A separate Client Catalog or
website may index published metadata without granting Host approval or
executing client code.

Studio remains the Host Operator control plane. It may discover and open an
approved compatible Activity Client for a provisioned Membership, but it does
not import Pack-specific participant renderers, impersonate another Room
Member, or receive participant-private Projection data merely to launch a
client. Studio may request a generic handoff and open the opaque launch target
returned by the Host, but its source does not select a client implementation,
interpret a client path, or embed client code. The user-facing action is **Open
participant client**, not “Open Participant View.”

Studio's Host Operator scope includes Pack installation and inspection, Room
creation and lifecycle, schema-driven Genesis configuration, Memberships,
Runner lifecycle, Agent Profiles, Runner Templates, Task Templates, integrity,
backup, and transfer. Studio may display generic contract metadata such as an
exact Pack identity, declared Roles and schemas, opaque Activity Phase labels,
Runner compatibility, and client readiness. It does not contain Pack-specific
field models, Projection decoders, renderer branches, client paths, or
executable imports. Installing a new Pack or approved client must not require a
Studio or Supervisor source-code change.

WorldStream does not execute third-party Activity Client or Client SDK code as
part of the Room Kernel or Studio. Clients execute independently and connect
through the scoped client contract. Hosting, installing, or sandboxing
third-party client code is a separate product and security boundary.

A later optional **Client Deployer** may retrieve an approved Activity Client
Release and start or publish it through a Docker, Kubernetes, static-web-host,
or other reviewed deployment adapter. It reports exact deployment identity and
readiness to the Client Binding Store. It remains outside the Room Kernel and
Studio execution boundaries. Studio may request generic deployment operations,
and a Host Operator may instead approve an externally managed Client
Deployment.

Deployment templates may declare bounded resource capabilities and secret
names. They contain no secret values or permission grants. A Host Operator
approves capabilities and supplies secrets to a Client Deployment through the
Client Deployer or an external deployment facility. Deployment secrets remain
outside Rooms, Activity Packs, Activity Distributions, and Membership-scoped
client credentials.

The first browser release accepts only exact Host Operator-approved loopback
origins. Remote HTTPS Activity Clients require a later origin, deployment, and
handoff security contract. CLI, SDK-built, and agent-owned Activity Clients
authenticate and connect directly; Studio does not launch them.

Remote HTTPS handoff is not an extension of the current local cookie design.
It requires a separate security ADR covering exact registered redirect targets,
origin-bound one-use authorization, short expiry, redirect handling, browser
cross-origin policy, and credential retention. An Activity Distribution or
Artifact Registry entry cannot enable remote launch before that contract is
accepted.

An Activity Client may perform local schema validation, accessibility behavior,
visualization, caching, optimistic animation, and other disposable interaction
work. Such behavior never establishes legality, commitment, hidden state, or an
Outcome. External effects initiated by a client remain outside WorldStream
truth unless an accepted Action or Host Stimulus records the relevant fact.

The first-party **WorldStream Inspector** is the Pack-neutral browser fallback.
It displays the exact authorized Projection/Observation data, Action Offers,
receipts, connection state, and Replay evidence supplied by the protocol. It
does not infer a Pack-specific experience or maintain a Pack renderer registry.
The Inspector is an Activity Client Release with an approved fallback Client
Deployment, not a wildcard Client Binding. Its launch target is Host
configuration and is not hardcoded in Studio or Supervisor source. When no
approved specialized client is compatible, the Host offers that deployment
rather than asking Studio to synthesize a Pack-specific interface. The absence
of a specialized browser client does not block CLI, SDK, or agent participation.

An open-source client conformance kit may produce immutable, content-addressed
**Client Conformance Evidence** for one exact Activity Client Release. It tests
declared client-contract versions, exact Pack compatibility, Access Modes,
Roles, Projection schema handling, Projection Reset replacement, reconnect
behavior, and separation of recorded fixtures from live authorized state.
Conformance evidence supports Host review but is neither a security audit nor
automatic approval.

The first implementation freezes the registry-neutral Release, Distribution,
Deployment, and Binding records without freezing automatic third-party client
execution or deployment:

- one trusted local Client Host may compile and serve separately identified
  Inspector, Agent Heist, and Negotiate Releases on separate paths under one
  approved loopback origin;
- that Client Host is a first-party development/deployment facility, not a
  universal plugin runtime; its source may know its own clients, while Studio
  and the Supervisor remain independent of those client implementations and
  paths;
- the Host resolves an approved Client Binding without a Pack-specific Studio
  or Supervisor source branch and otherwise offers the Inspector;
- the existing one-use fragment handoff and retained HttpOnly
  Membership-scoped session remain the launch authority seam;
- Agent Heist recorded and live modes share presentation modules but use
  disjoint adapters; and
- live mode starts without Activity data, installs only an authorized
  Projection Reset/Observation, replaces omitted state instead of falling back
  to fixture data, and exposes no client-side Projection-lens switch.

The existing `participant-console` route names, wire versions, and release
artifact names remain compatibility names during this migration. They do not
change the domain boundary established here.

## Consequences

- Pack Authors can define rules without learning a frontend ABI. Application
  Integrators can build domain-specific clients in any language that implements
  the ordinary client contract.
- A UI-only release does not create a new Activity Pack Revision, and a new
  Pack revision is not assumed compatible with an older client by name or
  semantic version.
- Studio and the Inspector contain no Pack-specific participant branches.
  Negotiate is an independent Activity Client using the same Binding and
  handoff path as Agent Heist and Inspector.
- A missing or incompatible specialized client fails closed to the Inspector;
  it does not prevent SDK, CLI, or agent participation.
- The local single-origin Client Host is a first-party release seam, not a
  general trust model for third-party executable code.
- Client Binding Store approvals and cached artifacts are Host-local
  operational trust. Room backup and transfer exclude them. A destination Host
  retrieves and approves its own Releases, Deployments, and Bindings.
- OCI artifact transport and exact-digest release identity are the long-term
  distribution seam. Automatic installation or deployment, publisher trust
  policy, mandatory signature enforcement, remote HTTPS handoff, sandboxed
  embedding, and a marketplace remain separate contracts and security
  milestones.

This decision refines ADR 0013's Studio/Participant Console distinction and
the presentation boundary in ADR 0014. Its long-term OCI artifact transport
also narrows ADR 0014's no-network-registry constraint without changing the
current `.wspack` approval identity or frozen release supply-chain graph. It
does not change the Room Kernel, ActivityPackV1, protocol authority, canonical
storage, or Replay semantics.

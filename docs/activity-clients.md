# Activity Clients

WorldStream keeps an Activity's rules and its presentation independent. An
Activity Pack is a headless, deterministic rule set executed by the Room
Kernel. An Activity Client is an independently executing browser, terminal,
mobile, service, SDK-built, or agent-owned application that consumes the
scoped client protocol.

This boundary lets one exact Pack revision have no specialized client, one
reference client, or several independently released clients. It also lets a
client support several exact Pack revisions. A client never gains Room
authority from being published, deployed, selected, or launched.

## What ships now

The repository contains three first-party browser clients:

| Client | Local surface | Purpose |
| --- | --- | --- |
| Agent Heist | `http://127.0.0.1:5173/agent-heist/` | Participant and spectator experience for exact Agent Heist revisions |
| Negotiate | `http://127.0.0.1:5173/negotiate/` | Participant and spectator experience for the exact Negotiate revision |
| WorldStream Inspector | `http://127.0.0.1:5173/inspector/` | Pack-neutral protocol workbench and configured fallback |

Studio is not one of these clients. Studio is the Host Operator plane: it
manages the daemon, Pack inventory, Room formation, Membership provisioning,
Runner setup, diagnostics, Replay, evidence, and approved client launch. It
contains no Pack-specific projection parser, route table, participant
renderer, or legality model.

Direct CLI, SDK, service, and agent clients may attach with their own scoped
authority and do not need Studio or a Client Binding. Bindings exist only for
Host-brokered launch.

## Release and distribution contracts

An `ActivityClientReleaseV1` document identifies one immutable,
content-addressed client build. It declares:

- a stable `client_id` and exact `release_digest`;
- the supported WorldStream client-contract version;
- exact artifacts and their media types and digests;
- one or more independently launched Client Surfaces; and
- exact, informative conformance-evidence references.

An `ActivityDistributionV1` document is a versioned integrator proposal. It
references one or more exact physical Activity Pack Bundles, exact Activity
Client Releases, surface/Pack/Access Mode/Role compatibility claims, and
optional Runner, documentation, or deployment-template artifacts. It grants
no Host approval, creates no Deployment, and changes no Room state.

The validated Rust contracts live in
`crates/worldstream-activity-client`. First-party examples live under
`config/activity-clients/releases/` and
`config/activity-clients/distributions/`.

The repository hashes built browser directories as exact runnable
local-development artifacts. The single-origin development host mounts each
Pack-specific directory at its declared entrypoint instead of rebundling it
inside Studio or Inspector. The sample local Deployments remain
`externally_trusted` because the Host has no attestation that those running
servers are serving the retained build digest. A production `verified`
Deployment must prove that its running bytes match the approved Release.

For long-term distribution, use OCI as the registry-neutral artifact transport
for immutable client payloads and manifests. npm, PyPI, and crates.io remain
appropriate for SDK and build-time libraries; they are not client-selection or
Host-approval authorities. Automated OCI publication, a Client Deployer, a
discovery Catalog, remote HTTPS launch policy, and a marketplace are deferred.

## Host-local deployment and selection

The Client Binding Store is Host-owned operational configuration. It retains
approved Releases, Deployments, Bindings, and an optional Inspector fallback.
It is not part of an Activity Distribution, Room backup, Genesis,
Authoritative Room State, canonical hashes, or Replay.

A Client Deployment makes one approved Release available at exact launch
targets and records whether those running bytes are `verified` or
`externally_trusted`. A Client Binding associates all of the following:

- semantic Pack Revision digest, with Pack ID and version retained as
  diagnostic labels;
- exact client-contract version;
- Access Mode;
- applicable participant Role set;
- approved Deployment and Client Surface; and
- optional default preference.

Selection always evaluates the current Membership. A stale Role, Access Mode,
standing, disabled Binding, revoked Deployment, disallowed Deployment Trust
Level, unready surface, or mismatched Pack digest fails closed. One eligible
candidate is selected directly. A
unique default wins among several candidates; otherwise Studio presents
opaque choices. When none match, the Host may offer its separately configured
Inspector fallback. Selection never relies on a mutable tag or a Pack-specific
branch in Studio or the Supervisor.

The local bootstrap in `config/activity-clients/local-bindings.json` explicitly
allows `externally_trusted` development Deployments, approves the three
first-party Deployments, binds exact Heist and Negotiate identities, and
configures Inspector explicitly. Disabling a Binding prevents new handoffs but
does not terminate an already retained session. Revoking a Deployment blocks
new handoffs and permanently invalidates sessions using that Deployment.
Lifecycle state is mutable and separate from immutable release and binding
records.

## Opaque browser handoff

For a Studio-opened human client:

1. Studio asks the Supervisor to resolve a client for the provisioned current
   Membership.
2. If a choice is required, Studio shows only generic Release and Surface
   metadata and submits the opaque selection ID.
3. The Supervisor issues a short-lived, origin-bound, one-use handoff for the
   selected approved surface.
4. The new browser surface removes the handoff from the URL and redeems it
   into a retained HttpOnly session.
5. The Supervisor revalidates the current Membership and selected Deployment
   before redemption and before every retained session operation.
6. The client attaches, installs only its authorized Projection Reset or
   Observation Frames, acknowledges the synchronization barrier, and then
   enables exact offered Actions.

Room IDs, Membership IDs, bearer credentials, filesystem paths, and launch
URLs do not pass through Studio's browser-visible selection payload. A
spectator session is read-only even if a specialized client exposes both
participant and spectator presentation.

## Client implementation rules

An Activity Client can be visually rich and Pack-aware while remaining
protocol-thin:

- start live state empty;
- atomically replace it with the authorized Projection Reset;
- consume only authorized Projection or Observation bytes and exact Action
  Offers;
- never infer server legality or canonical state;
- keep participant, spectator, and operator access paths distinct;
- disable Actions during reconnect or an unsatisfied reset barrier;
- treat Replay as read-only; and
- submit the same typed protocol Actions as any other client.

Recorded demos must use a separate fixture adapter. They may reuse
presentation components, but a live adapter must never initialize from or
overlay fixture data. The Agent Heist gallery and live client enforce this
separation.

The shared `@worldstream/client` package supplies generic transport and
retained-session behavior. It is a Client SDK, not an Activity Client or a
runtime plugin. Client authors remain free to use another language or protocol
implementation.

## Local development

Start the Client Host:

```sh
pnpm ui:dev
```

This command rebuilds and serves the three exact retained directories on one
origin. Use `pnpm ui:source-dev` only for HMR work on the recorded gallery and
Inspector source; it does not host the standalone Heist or Negotiate Releases.

Start Studio and its Supervisor separately as documented in
[Getting started](getting-started.md). Studio launches the approved surface;
opening a path directly does not provision Room authority.

Run the complete first-party conformance lane:

```sh
pnpm activity-clients:verify
```

The lane checks Rust formatting, rebuilds the runnable artifacts, runs the full
Rust workspace and browser acceptance, verifies exact Release,
Distribution, Deployment, Binding, fallback, and Release-bound evidence
identities; checks Studio, Supervisor, and Inspector Pack neutrality; runs the
client, Studio, Client Host, focused Rust, and documentation gates; and proves
the exact hosted live artifacts start empty. Its evidence is informative only.
It is not a security audit, proof of production-deployed bytes, or Host
approval.

When a first-party client changes, rebuild before printing the newly derived
local identities and updating reviewed Release references:

```sh
pnpm activity-clients:build
node scripts/verify-activity-clients.mjs --print-identities
```

The checked-in Negotiate Distribution references its qualified physical
`.wspack` Bundle. Agent Heist is still an embedded Runtime Pack, so this tree
does not pretend it has a separately published physical Pack Bundle or publish
an Agent Heist Distribution manifest yet.

## Adding a first-party client

1. Build an independently executable client against the generic client
   contract. Keep its live state empty until an authorized reset arrives.
2. Add tests for reset replacement, reconnect, exact Action Offers, Replay,
   Access Mode/Role privacy, and recorded/live separation where applicable.
3. Add an immutable Release document and exact conformance-evidence link.
4. Optionally add an Activity Distribution that references the exact physical
   Pack Bundle, Release, compatibility claims, and optional integration
   artifacts without merging their authority domains.
5. Have the Host Operator approve a Deployment and one or more exact Bindings.
6. Run `pnpm activity-clients:verify` and the relevant production build.

Do not add a Pack route, projection decoder, or renderer to Studio. Do not put
client code or a mutable URL in the Pack Bundle. Do not make the Inspector a
generated UI framework for domain experiences.

The normative boundary is frozen in
[ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md). The
research that led to it is retained in
[Pack-associated clients and participant surfaces](pack-client-surfaces-research.md).

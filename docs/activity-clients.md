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

## Preserved browser examples

The loopback development host serves four current source builds:

| Client | Path on `http://127.0.0.1:5173` | Demonstrates |
| --- | --- | --- |
| Agent Heist | `/agent-heist-v12/` | Hidden-information participant and spectator views |
| Negotiate | `/negotiate-v3/` | Typed offers and approvals |
| Midnight Archive | `/midnight-archive-v14/` | Cooperative game state and offered Actions |
| Inspector | `/inspector-v2/` | Pack-neutral projections, observations, and receipts |

These are local usage examples. Historical compiled browser assets and the
hosted platform have been removed. The release declarations in the example
catalog remain as test fixtures and templates; they do not approve newly built
bytes or imply an available deployment.

The authenticated CLI is the Host Operator plane. The headless Controller
manages Runtime lifecycle, Room formation, scoped connection preparation, and
approved client launch. Neither contains a Pack-specific projection parser,
route table, participant renderer, or legality model.

Direct CLI, SDK, service, and agent clients may attach with their own scoped
authority and do not need the Controller or a Client Binding. Bindings exist only for
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
`examples/clients/catalog/releases/` and
`examples/clients/catalog/distributions/`.

The repository hashes built browser directories as exact runnable
local-development artifacts. The single-origin development host mounts each
Pack-specific directory at its declared entrypoint instead of rebundling it
inside the Controller or Inspector. The sample local Deployments remain
`externally_trusted` because the Host has no attestation that those running
servers are serving the retained build digest. A production `verified`
Deployment must prove that its running bytes match the approved Release.

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
unique default wins among several candidates; otherwise the CLI presents
opaque choices. When none match, the Host may offer its separately configured
Inspector fallback. Selection never relies on a mutable tag or a Pack-specific
branch in the CLI or Controller.

The local bootstrap in `examples/clients/catalog/local-bindings.json` explicitly
allows `externally_trusted` development Deployments, approves the first-party
Deployments, binds exact Heist and Negotiate identities, and
configures Inspector explicitly. Disabling a Binding prevents new handoffs but
does not terminate an already retained session. Revoking a Deployment blocks
new handoffs and permanently invalidates sessions using that Deployment.
Lifecycle state is mutable and separate from immutable release and binding
records.

## Opaque browser handoff

For a CLI-opened human client:

1. `worldstreamctl client open` asks the Controller to resolve a client for the provisioned current
   Membership.
2. If a choice is required, the CLI shows only generic Release and Surface
   metadata and submits the opaque selection ID.
3. The Controller issues a short-lived, origin-bound, one-use handoff for the
   selected approved surface.
4. The new browser surface removes the handoff from the URL and redeems it
   into a retained HttpOnly session.
5. The Controller revalidates the current Membership and selected Deployment
   before redemption and before every retained session operation.
6. The client attaches, installs only its authorized Projection Reset or
   Observation Frames, acknowledges the synchronization barrier, and then
   enables exact offered Actions.

Room IDs, Membership IDs, bearer credentials, filesystem paths, and launch
URLs do not pass through the CLI's generic selection report. A
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

From the repository root:

```sh
pnpm install --frozen-lockfile
pnpm activity-clients:check
pnpm activity-clients:serve
```

This rebuilds and serves the four current clients on one loopback origin.
A live participant also needs a running local Controller, a Room Membership,
and an authorized handoff. Opening a client URL alone does not join a Room.

The source defaults use Controller origin `http://127.0.0.1:9420`. Changing that
origin changes client bytes and therefore requires new release declarations.
`scripts/prepare-local-activity-client.mjs` hashes an exact local build and
prepares declarations for operator review and import. The operator CLI owns
approval; the browser build does not grant itself authority.

Hosted transport adapters remain in some example sources for reference. Their
former authentication service, gateway, and database are retired; the hosted
entrypoints are not working product demos. See the [client examples](../examples/clients/README.md)
and [local operator guide](getting-started.md).

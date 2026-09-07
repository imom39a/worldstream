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
| Agent Heist | `http://127.0.0.1:5173/agent-heist-v2/` | Participant and spectator experience for exact Agent Heist revisions |
| Negotiate 0.1 | `http://127.0.0.1:5173/negotiate/` | Retained immutable participant and spectator client for the exact 0.1 revision |
| Negotiate 0.2 | `http://127.0.0.1:5173/negotiate-v2/` | Current participant and spectator client for the exact 0.2 revision |
| WorldStream Inspector | `http://127.0.0.1:5173/inspector/` | Pack-neutral protocol workbench and configured fallback |

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
`config/activity-clients/releases/` and
`config/activity-clients/distributions/`.

The repository hashes built browser directories as exact runnable
local-development artifacts. The single-origin development host mounts each
Pack-specific directory at its declared entrypoint instead of rebundling it
inside the Controller or Inspector. The sample local Deployments remain
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
unique default wins among several candidates; otherwise the CLI presents
opaque choices. When none match, the Host may offer its separately configured
Inspector fallback. Selection never relies on a mutable tag or a Pack-specific
branch in the CLI or Controller.

The local bootstrap in `config/activity-clients/local-bindings.json` explicitly
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

Start the Client Host:

```sh
pnpm ui:dev
```

This command rebuilds the three current first-party builds and serves four
exact artifact directories on one origin. The fourth directory is the retained
immutable Negotiate 0.1 build. Use `pnpm ui:source-dev` only for HMR work on the
recorded gallery and Inspector source; it does not host the standalone Heist or
Negotiate Releases.

The checked-in Releases and Deployments use Controller origin
`http://127.0.0.1:9420` and Client Host origin `http://127.0.0.1:5173`. A
different Controller changes the built client bytes. It therefore requires a
new exact local Release and Binding; changing only the Host Content Security
Policy does not redirect a client.

Agent Heist v2 has two explicit entrypoints in the same immutable artifact.
`/agent-heist-v2/` is the local kernel client used by `worldstreamctl client open`.
`/agent-heist-v2/hosted/` requires the authenticated hosted Platform and direct
Gateway stream. It does not fall back to local authority when sign-in fails.
The earlier `/agent-heist/` path returns 404 unless an operator separately
retains and serves the exact original artifact. Existing records are not
silently rebound to the new client.

For example, prepare a fresh-installation Agent Heist client for Controller
`19420` and Client Host `15173` as follows. Build only the selected client and
its Inspector fallback so unrelated checked-in client artifacts stay exact:

```sh
VITE_WORLDSTREAM_SUPERVISOR_URL=http://127.0.0.1:19420 pnpm heist-client:build
VITE_WORLDSTREAM_SUPERVISOR_URL=http://127.0.0.1:19420 pnpm ui:build

node scripts/prepare-local-activity-client.mjs \
  --dist clients/agent-heist-web/dist \
  --source-release config/activity-clients/releases/agent-heist-web-v2.json \
  --pack-id worldstream.agent-heist \
  --pack-version 0.2.0 \
  --pack-digest blake3:b1fc05278808c854c3b97c03639196d6d223a66f283649fa4d349fa477e4b820 \
  --access-mode participant \
  --roles navigator,insider,broker \
  --surface heist-web \
  --entrypoint /agent-heist-v2/ \
  --launch-url http://127.0.0.1:15173/agent-heist-v2/ \
  --fallback-dist web/console/dist \
  --fallback-source-release config/activity-clients/releases/inspector-web.json \
  --fallback-surface inspector-web \
  --fallback-entrypoint /inspector/ \
  --fallback-launch-url http://127.0.0.1:15173/inspector/ \
  --output .worldstream/heist-client-19420
```

The output directory must be new or empty. Review and import its
`client-declaration.json` through the exact two-phase `worldstreamctl init`
flow before Controller startup. Do not first import the checked-in default
binding into this fresh installation. Then serve the already reviewed bytes:

```sh
node scripts/serve-activity-clients.mjs \
  --port 15173 \
  --controller-origin http://127.0.0.1:19420
```

Start the Controller with the same two origins:

```sh
worldstreamctl server start \
  --controller 127.0.0.1:19420 \
  --participant-console-origin http://127.0.0.1:15173
```

The build-time `VITE_WORLDSTREAM_SUPERVISOR_URL` and Host
`--controller-origin` must name the same Controller. The prepared Deployment
launch URLs and Runtime `--participant-console-origin` must name the same
Client Host. All values must be exact loopback origins. A path, trailing slash,
remote host, or different port fails closed. Prepare and import another exact
declaration for each additional client surface that this installation will
select.

Start the Controller and Runtime with `worldstreamctl server start`, then use
`worldstreamctl client open` for the selected operation and seat as documented
in [Getting started](getting-started.md). Opening a path directly does not
provision Room authority.

Run the complete first-party conformance lane:

```sh
pnpm activity-clients:verify
```

The lane checks Rust formatting, rebuilds the runnable artifacts, runs the full
Rust workspace and browser acceptance, verifies exact Release,
Distribution, Deployment, Binding, fallback, and Release-bound evidence
identities; checks Controller and Inspector Pack neutrality; runs the
clients, Client Host, focused Rust, and documentation gates; and proves
the exact hosted live artifacts start empty. Its evidence is informative only.
It is not a security audit, proof of production-deployed bytes, or Host
approval.

When a first-party client changes, rebuild before printing the newly derived
local identities and updating reviewed Release references:

```sh
pnpm activity-clients:build
node scripts/verify-activity-clients.mjs --print-identities
```

The checked-in Negotiate 0.1.0 Distribution remains bound to its historical
Pack and client Releases. The separate 0.2.0 Distribution references the
corrected physical Pack Bundle, its successful production Component Host/Core
proof, and the newer client Release that explicitly supports both exact Pack
revisions. The 0.2.0 proof does not claim durable-Room restart, deployed-client
byte verification, Host approval, or broader release qualification. Agent
Heist is still an embedded Runtime Pack, so this tree does not pretend it has a
separately published physical Pack Bundle or publish an Agent Heist
Distribution manifest yet.

The Client Host retains the exact original Negotiate artifact
`sha256:ba00f3e8f72a5012ce6e9018c457ec36d70a0809ceb5e421b946006f50855429`
at its immutable `/negotiate/` entrypoint. It serves the newer Release at the
separate `/negotiate-v2/` entrypoint. The conformance lane hashes each mounted
directory and rejects a Deployment whose launch target does not serve the
artifact declared by its exact Release.

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

Do not add a Pack route, projection decoder, or renderer to the CLI or Controller. Do not put
client code or a mutable URL in the Pack Bundle. Do not make the Inspector a
generated UI framework for domain experiences.

The normative boundary is frozen in
[ADR 0017](adr/0017-separate-activity-clients-from-packs-and-studio.md). The
research that led to it is retained in
[Pack-associated clients and participant surfaces](pack-client-surfaces-research.md).

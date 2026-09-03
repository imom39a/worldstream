# Activity Clients

An Activity Pack defines a Room's deterministic rules. An Activity Client is
an independent browser, terminal, mobile, service, SDK-built, or agent-owned
application that presents and participates in that Room through scoped
WorldStream protocol authority.

The operator CLI does not host these experiences. Its headless Controller can
broker an approved launch, while Pack-specific rendering,
Projection interpretation, and participant interaction stay in the separate
client process.

## First-party browser surfaces

Start the local Client Host with:

```sh
pnpm ui:dev
```

The command rebuilds and mounts each exact standalone client directory on one
loopback origin. It does not rebundle Heist or Negotiate into the Controller or the
Inspector/recorded-gallery artifact.

It exposes:

| Surface | Local path | Responsibility |
| --- | --- | --- |
| Agent Heist | `/agent-heist/` | exact Heist participant or spectator experience |
| Negotiate 0.1 | `/negotiate/` | retained exact 0.1 participant or spectator experience |
| Negotiate 0.2 | `/negotiate-v2/` | current exact 0.2 participant or spectator experience |
| WorldStream Inspector | `/inspector/` | Pack-neutral authorized protocol workbench and configured fallback |

Opening one of these paths does not create a Membership or grant authority.
The normal browser flow starts with `worldstreamctl client open`. The
Controller selects an approved surface for the current Membership and brokers
a short-lived, origin-bound, one-use handoff into an HttpOnly retained session.

## Release, deployment, and binding

An Activity Client Release is an immutable, content-addressed build declaring
its protocol contract, exact runnable artifacts, surface entrypoints,
capabilities, and Release-bound informative conformance evidence. An Activity
Distribution may reference exact physical Pack Bundles, Client Releases,
compatibility claims, and optional integration artifacts, but publishing or
importing it does not approve or run anything.

The Host-local Client Binding Store separately records:

- approved Releases;
- exact running Client Deployments and their `verified` or
  `externally_trusted` trust level;
- Client Bindings from an exact semantic Pack revision digest (with ID/version
  as diagnostic labels), client contract, Access Mode, and Role set to an
  approved Deployment Surface; and
- an optional explicitly configured Inspector fallback.

Selection evaluates the current Membership before every retained operation
and fails closed on stale Role or Access Mode, disabled Bindings, revoked or
unready Deployments, disallowed trust levels, and any exact identity mismatch.
With several eligible surfaces, a unique default wins or
the CLI presents opaque generic choices. The Controller never selects through a
Pack-specific route branch.

Disabling a Binding prevents new handoffs without terminating an already
retained session. Revoking its Deployment prevents new handoffs and
permanently invalidates affected retained sessions.

The checked-in local model is under `config/activity-clients/`. Release
artifacts identify exact built browser directories. Local development
Deployments are still
`externally_trusted` because this development workflow does not attest that
the running server is serving those retained bytes.

## Client safety contract

A live client starts with no Activity data. It installs an authorized
Projection Reset atomically, applies only authorized Observation Frames, and
renders only exact current Action Offers. It does not reconstruct legality,
read the database, or hold canonical state. Actions stay disabled across a
stale or unsatisfied synchronization barrier, and Replay remains read-only.

Recorded demonstrations use a separate fixture adapter. Presentation
components may be shared, but live state must never initialize from or overlay
recorded fixture values. Agent Heist tests this boundary explicitly.

Direct CLI, SDK, service, and agent clients may attach with their own scoped
authority; they do not need the Controller or a Client Binding.

## Verify the contract

```sh
pnpm activity-clients:verify
pnpm heist-client:build
pnpm negotiate-client:build
pnpm ui:build
```

The conformance lane checks Rust formatting, rebuilds and checks exact local
Release, Distribution,
Deployment, Binding, fallback, and Release-bound evidence identities; enforces
CLI, Controller, and Inspector Pack neutrality; and runs client,
Client Host, focused and full-workspace Rust, browser, and documentation gates.
Passing is informative evidence, not a security audit, production
deployment-byte proof, or Host approval.

OCI is the planned registry-neutral transport for immutable production client
artifacts. npm, PyPI, and crates.io remain SDK and build-library channels, not
selection or approval authorities. Automated OCI publishing, remote HTTPS
launch, a Client Deployer, a Catalog, and marketplace policy are later work.

Source: [Activity Client architecture](https://github.com/imom39a/worldstream/blob/main/docs/activity-clients.md)
and [ADR 0017](https://github.com/imom39a/worldstream/blob/main/docs/adr/0017-separate-activity-clients-from-packs-and-studio.md).

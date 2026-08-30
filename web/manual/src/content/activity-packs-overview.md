# Activity Pack development

An Activity Pack defines deterministic rules for a class of Rooms. Public Pack
Authors use one code-first TypeScript project; the deployment target is a
WASI-free WebAssembly Component, not a required source language.

## One semantic contract

Every revision implements exactly `descriptor`, `initialize`, `reduce`, `view`,
and `observe`. Embedded Rust revisions and portable Components pass through the
same checked host. A Room pins one immutable `PackRevisionLockV1` digest and
never upgrades in place.

Packs own Activity State, Roles, typed Actions and Action Offers, deterministic
reduction, viewer-scoped Projections/observations, phases, timers, Attention,
and Outcomes. They do not own Membership, Room ordering, persistence, Replay,
model execution, prompts, tools, credentials, storage, network, or signing
keys.

## Public bundle

One deterministic `.wspack` contains the exact Component, revision lock,
descriptor, schemas, codec declaration, dependency lock, immutable static
material, golden corpus, and conformance facts. Its physical bundle BLAKE3 is
separate from the semantic revision digest.

The Host Operator inspects and approves one exact bundle digest, installs it
offline, and restarts the daemon. Startup assembles one immutable registry from
embedded revisions and approved bundles. There is no network registry, hot
loading, name/version fallback, automatic approval, or force removal of a
retained revision.

The first execution profile has exactly five exports, zero imports, no WASI
linker, synchronous calls, fresh instances, and fixed revision-bound limits.
Original Component bytes remain authoritative; generated AOT cache is
disposable. Failure commits nothing.

## Offline installation and startup seal

Keep `worldstreamd` stopped and use one reviewed configuration for the whole
operator lifecycle:

```sh
worldstreamctl --config <WORLDSTREAM_CONFIG> pack inspect --bundle <PACK.wspack>
worldstreamctl --config <WORLDSTREAM_CONFIG> pack approve --bundle <PACK.wspack> <APPROVAL_ARGUMENTS>
worldstreamctl --config <WORLDSTREAM_CONFIG> pack install --bundle <PACK.wspack> <INSTALL_ARGUMENTS>
worldstreamctl --config <WORLDSTREAM_CONFIG> pack inventory
worldstreamctl --config <WORLDSTREAM_CONFIG> pack set-selectable --bundle-digest <BUNDLE_DIGEST> --selectable true
worldstreamctl --config <WORLDSTREAM_CONFIG> pack inventory
worldstreamctl --config <WORLDSTREAM_CONFIG> pack restart-readiness
```

For `postgres-primary`, append the owner-only admin boundary:

```sh
worldstreamctl --config <WORLDSTREAM_CONFIG> pack restart-readiness \
  --dsn-file <POSTGRES_ADMIN_DSN_FILE>
```

Inventory and readiness receipts are config-aware and carry
`storage_profile`. The post-selection inventory and readiness
`inventory_digest` must match. Restart readiness admits original Component
bytes through the production host, executes exact Replay against the configured
store, derives a pathless `deployment_binding` from the canonical data
directory, profile, and available provider metadata, then writes a durable
startup-readiness seal over that exact inventory/target pair.

At startup the daemon derives the binding again and refuses installed portable
bundles when the seal is absent or differs. Install, approval revocation,
selectability change, retained-bundle restore, and removal clear the seal, so
run `restart-readiness` again after any such mutation. A browser receipt does
not replace this daemon-enforced seal.

## Author workflow

The offline, code-first path is canonical:

```sh
npx @worldstream/pack-cli@<release> new <directory>
npm ci
npm run pack:check
npm run pack:test
npm run pack:build
npm run pack:inspect
npm run pack:prove
```

Prompt assistance can create the same project schema from a bounded description
through an OpenAI-compatible endpoint:

```sh
WORLDSTREAM_PACK_OPENAI_ENDPOINT=https://provider.example/v1/chat/completions \
WORLDSTREAM_PACK_OPENAI_MODEL=<model> \
WORLDSTREAM_PACK_OPENAI_API_KEY=<key> \
npx @worldstream/pack-cli@<release> create <directory> --prompt "<description>"

# Review every staged file, then use the exact receipt values:
npx @worldstream/pack-cli@<release> create <directory> --confirm <review-id>
```

The TypeScript SDK/CLI is an implementation frontier, not yet a shipped
capability. Authors edit `worldstream-pack.json`, `src/pack.ts`, golden/privacy
fixtures, and tests. WorldStream tooling owns WIT, generated bindings, schemas,
codecs, Componentization, digests, revision locks, and proof evidence.

`pack:check` will run strict TypeScript checks, reject ambient clock/random/
filesystem/network/process/timer/dynamic-code access and mutable module
authority, force Componentization with all capabilities disabled, preflight
exact imports/exports, and smoke all five callbacks through the production
host. `pack:prove` will add restart, resource, privacy, retained-revision, and a
real local Room proof.

Optional prompt assistance is required for the first release but is deliberately
bounded. The provider returns only a strict starter/display-name/description
blueprint; WorldStream renders the fixed files below a private staging root.
The first command returns `review_required`, and only a separate `--confirm`
promotes the exact reviewed bytes. It neither installs packages nor runs the
pipeline automatically.

Provider output cannot supply source, paths, dependencies, shell commands, or
hidden executable rules. Symlinks, unknown files, changed staged bytes, and
destination escape are rejected. Provider response bodies and the endpoint,
model, key, and prompt transcript never enter the daemon, project, bundle,
evidence, receipts, or normal logs. Both offline and assisted projects use the
same `check`, `test`, `build`, `inspect`, and `prove` commands.

## Current activities

- [WorldStream Negotiate](#/activity-packs/negotiate) is the first serious
  public Pack and must dogfood the public TypeScript Component path.
- [Agent Heist](#/activity-packs/agent-heist) is a visual demo/conformance Pack.
- [Counter](#/activity-packs/counter) is the internal walking skeleton and
  tutorial; it is not the product story.
- Investigation Room is deferred design research.

Primary sources: [Activity Pack contract](https://github.com/imom39a/worldstream/blob/main/docs/activity-packs.md),
[bundle ADR](https://github.com/imom39a/worldstream/blob/main/docs/adr/0014-installable-wasi-free-activity-pack-bundles.md),
and [roadmap](https://github.com/imom39a/worldstream/blob/main/docs/roadmap.md).

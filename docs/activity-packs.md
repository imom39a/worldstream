# Activity Pack Design

## Status

This is the frozen semantic `ActivityPackV1` host contract plus the accepted portable Activity Pack Bundle lifecycle. [WorldStream Negotiate](../examples/negotiate/design.md) is one retained example.

`ActivityPackV1` remains one stable semantic contract for every retained Room. Public Pack revisions implement its five operations in a WebAssembly Component without WASI. Embedded Rust implementations provide internal adapters and reference implementations.

The host seam and executable-retention decision are accepted in [ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md), as partially superseded by the bundle/execution decision in [ADR 0014](adr/0014-installable-wasi-free-activity-pack-bundles.md).

## Purpose

An Activity Pack defines what one room means:

- configuration and Canonical Activity State;
- participant roles;
- typed actions and Action Offer rules;
- deterministic state transitions;
- public, participant, and operator projections;
- timers and attention signals;
- completion conditions and scoring.

WorldStream defines how the room is ordered, persisted, recovered, streamed, reattached, activated, and replayed.

One Room pins exactly one Activity Pack Revision. Packs cannot call each other or mutate another Room in this implementation.

## When an activity fits

An activity is a good fit when:

- two or more independent participants affect shared state;
- roles or visibility differ;
- participants select actions rather than following a fixed server-authored workflow;
- actions can conflict or change Action Offers;
- timers, reconnect, recovery, or replay matter.

An activity is a poor fit when it is merely:

- a one-agent task wrapper;
- a connector or ETL step;
- a predetermined process graph;
- a document collection;
- a model or tool execution sandbox.

The pack author still owns the domain model. WorldStream is valuable only if its room semantics remove substantial repeated infrastructure. This Activity Pack tax is an explicit validation risk.

## Activity Pack Bundle contents

One canonical `.wspack` is a deterministic uncompressed ustar archive with
sorted paths, zeroed metadata, regular files only, and bounded members. It
contains:

    bundle-manifest.json
    revision-lock.json
    descriptor.json
    schemas.json
    codec-bundle.json
    executor.component.wasm
    golden-corpus.json
    conformance.json
    dependency-lock.json
    static/*                    optional, declared immutable material

The external BLAKE3 of every exact archive byte is the Activity Pack Bundle
identity; the archive cannot self-hash. Every member size/digest is declared
and independently verified. Absolute/traversal/duplicate paths, links, devices,
undeclared members, and mutable directories are rejected.

It does not contain:

- an LLM or model provider SDK;
- agent prompts or private memory;
- filesystem, database, network, shell, wallet, or secret access;
- workflow nodes;
- HTML or arbitrary JavaScript;
- another Activity Pack;
- cross-room references.

## Descriptor and revision identity

The descriptor declares pack identity, schemas, ordered Role and Action definitions, Attention reasons, projection variants, and hard output bounds. Principal kind and Role remain separate; WorldStream Core owns current Membership standing, Access Mode, and Role assignment.

~~~json
{
  "pack_id": "worldstream.agent-heist",
  "name": "Agent Heist",
  "explanatory_version": "0.1.0",
  "revision_digest": "blake3:...",
  "host_contract": "worldstream/activity-pack/v1",
  "canonical_codec": "worldstream/canonical-json/v1",
  "configuration_schema": "agent-heist/config/v1",
  "state_schema": "agent-heist/state/v1",
  "stimulus_schemas": [],
  "output_schemas": [],
  "roles": [],
  "actions": [],
  "attention_reasons": [],
  "projection_schemas": {},
  "limits": {
    "maximum_state_bytes": 2097152,
    "maximum_events": 128,
    "maximum_timer_requests": 32,
    "maximum_attention_signals": 32,
    "maximum_projection_bytes": 262144,
    "maximum_observation_bytes": 262144,
    "maximum_nesting": 32,
    "maximum_collection_items": 4096,
    "maximum_text_bytes": 65536
  }
}
~~~

The explanatory version is for people. Only the semantic revision digest selects executable rules. A Pack may retain immutable Genesis seat identities in Activity State. It MUST NOT store a second mutable index of current Role ownership. Reduction reads current assignments from the supplied Core view.

The Activity Pack defines Role names, cardinality, permissions, and Action Offers. The WorldStream Core reducer exclusively records current assignment in the semantic Membership map. Pack Activity State MUST NOT persist a second role-to-Membership ownership index; reduction derives any needed lookup from the supplied immutable Core view.

The revision digest pins the exact behavior and schemas. For a portable
revision, `PackRevisionLockV1.rule_source_digest` is the exact original
Component BLAKE3. A semantic version is explanatory; Replay trusts the Room's
semantic digest, and installation/transfer trust the separate exact bundle
digest.

## Core boundary seen by packs

`CoreRoomState v1` is host-owned and contains exactly Room Status plus a canonically sorted Membership map. For each Membership, it exposes the immutable Member ID, Principal ID, and room-local Principal kind. It also exposes standing and Access Mode. A Role is present only for participant access. Room Head, hashes, integrity, Sessions, Cursors/Frames, receipts, policy/Activation, diagnostics, telemetry, and commit time are not Core.

For every reduction, the host supplies immutable Core-before and proposed-Core-after values. They are equal for a non-Core Stimulus. A versioned Core Stimulus carries authority attribution, idempotency identity, exact expected sequence, reason code, semantic time when applicable, and a canonical before/after changeset. One Stimulus may change several Memberships atomically. The components use Member ID order, with at most one typed component per ID. The Pack observes only the complete proposed final state.

The pack may declare a stable veto for join, resume, Access Mode, or Role proposals. It may not veto archive, suspend, or depart; attempting to do so is a Pack Fault. A homogeneous multi-Membership changeset inherits the veto class of its typed components. The host rejects a changeset that mixes vetoable and mandatory component kinds before pack entry, with no pack call, Transition, or receipt. A veto produces an idempotent administrative rejection with no Transition, not an Activity Fault. Packs may change Activity State or emit deterministic outputs in response to an accepted Core change, but cannot mutate Core itself.

## ActivityPackV1

The complete trusted host seam is:

~~~rust
pub trait ActivityPackV1: Send + Sync + 'static {
    fn descriptor(&self) -> &PackRevisionDescriptorV1;

    fn initialize(
        &self,
        input: &GenesisInputV1,
        cx: &DeterministicContextV1,
    ) -> Result<InitialOutputV1, PackFault>;

    fn reduce(
        &self,
        input: &ReduceInputV1,
        cx: &DeterministicContextV1,
    ) -> Result<ReduceDispositionV1, PackFault>;

    fn view(&self, input: &ViewInputV1)
        -> Result<PackViewV1, PackFault>;

    fn observe(&self, input: &ObserveInputV1)
        -> Result<Option<PackObservationV1>, PackFault>;
}
~~~

There are exactly five operations: descriptor, initialize, reduce, view, and observe. Typed implementation helpers may exist behind this boundary, but no sixth semantic callback or pack-name branch is part of v1.

For a portable Component, the `descriptor` export returns the canonical
descriptor content with only `revision_digest` omitted. This avoids a hash
cycle: exact Component bytes determine `rule_source_digest`, the complete lock
then determines `revision_digest`, and `descriptor.json` retains the full
descriptor. The Component Host compares the exported content byte-for-byte
with the full descriptor's content form before its adapter enters this trait.

Every callback is pure, synchronous, bounded, and complete before persistence handoff. The pack receives no storage, network, filesystem, environment, scheduler, HostClock, database clock, Session, delivery, telemetry, Activation, Runner, model, wallet, secret, or artifact-byte capability. Same-process Rust remains trusted and is not a sandbox.

### DeterministicContextV1

The context exposes only canonical-value utilities and domain-separated labeled randomness derived from Room seed, exact pack digest, next Room sequence, label, and index. It exposes no ambient entropy or time. Integer and fixed-point operations are permitted; floating-point canonical state is forbidden.

The inputs include the revision lock, Genesis, Core inputs, timer view, and ordered normalized Stimuli. With identical inputs, every operation MUST return byte-identical canonical outputs on all supported platforms and storage profiles.

## Initialization

GenesisInputV1 contains exactly the recorded creation inputs visible to the pack:

- Room ID and exact revision digest;
- canonical pack configuration;
- immutable initial Core view, including the initial Membership identities and Roles;
- Room seed;
- recorded logical creation time.

InitialOutputV1 contains:

- one canonical initial Activity State value;
- one ordered list of TimerRequestV1 values.

The host schema-validates and canonicalizes the result, assigns and verifies timer generations, and records the normalized initial timer set. Genesis binds the initial state and timers. Initialization emits no Domain Event, Attention Signal, Activation, Observation Frame, or delivery side effect.

## Reduction input and normalized Stimulus

ReduceInputV1 contains:

- prior canonical Activity State;
- exact immutable Core before;
- exact immutable proposed Core after;
- the current scheduled timer view sorted by logical timer ID;
- next Room sequence;
- one normalized typed StimulusV1.

The pack cannot mutate either Core view. WorldStream produces proposed Core after by applying its own versioned Core reducer before pack reduction.

Normalized StimulusV1 has exactly four variants:

~~~text
ParticipantAction {
  member_id,
  action_id,
  action_type,
  payload_schema_digest,
  canonical_payload,
  exact_basis_head,
  admitted_at
}

TimerFired {
  timer_id,
  generation,
  scheduled_for,
  canonical_payload
}

CoreProposed {
  kind,
  canonical_authority_attribution,
  operation_identity,
  expected_room_seq,
  reason_code,
  recorded_at,
  canonical_changeset
}

ExternalInput {
  source_id,
  input_id,
  input_type,
  recorded_at,
  canonical_payload,
  immutable_resource_references
}
~~~

CoreProposed kinds are Join, Resume, AccessModeChange, RoleChange, MembershipChangeSet, Archive, Suspend, and Depart. `canonical_changeset` has this exact shape:

~~~text
CoreChangeSet {
  room_status_change: null | {
    before,
    after
  },
  membership_changes: [
    {
      kind,
      member_id,
      before,
      after
    }
  ]
}
~~~

Each Membership component kind is exactly Join, Resume, AccessModeChange, RoleChange, Suspend, or Depart. `Archive` is only its own top-level CoreProposed kind: it carries the active-to-archived Room Status pair and no Membership component, and it cannot appear inside MembershipChangeSet. An individual Membership CoreProposed kind carries exactly one matching typed component and no Room Status change. `MembershipChangeSet` is the atomic multi-Membership form. It carries no Room Status change. It sorts components by Member ID and contains at most one component per Member ID. The complete changeset MUST match the delta between the immutable Core-before and proposed-Core-after values in ReduceInput. The Host rejects a mismatch before Pack entry. Host-normalized NoChange remains a pre-pack disposition and never enters the pack.

Join, Resume, AccessModeChange, and RoleChange are vetoable component kinds. Suspend and Depart are mandatory component kinds. The host rejects a MembershipChangeSet containing both classes before pack entry, with no pack call, Transition, or receipt. A homogeneous changeset inherits the class of its components. The Pack may reject an all-vetoable set only as one complete atomic proposal. It must apply an all-mandatory set as one complete atomic proposal. A Reject in that case is PackFault.

TimerFired contains only the exact timer identity, immutable scheduled_for, and canonical payload; detection, lag, retry, database, and commit times are excluded. ParticipantAction carries host-recorded admitted_at. CoreProposed and ExternalInput carry canonical recorded_at. These typed fields supply semantic time; there is no universal Transition timestamp.

ExternalInput is narrow, authenticated, idempotent, and predefined by the release. In v0.1 it is not a connector, arbitrary artifact reader, callback, or general effect mechanism. Any future immutable resource reference is already authorized and resolved before pack entry.

## Reduction dispositions and Core veto

ReduceDispositionV1 is exactly:

~~~text
Apply {
  next_state,
  ordered_domain_events,
  timer_requests,
  attention_signals
}

Reject {
  declared_code,
  bounded_safe_details
}
~~~

PackFault is the operation error channel, not a domain disposition.

Apply returns the complete next canonical Activity State and ordered canonical outputs. It produces one Transition even when the resulting Activity State bytes equal the prior bytes. Domain Events are ordered within that Transition and receive no independent Room sequence. Private historical audiences use immutable Membership IDs, never a mutable Role lookup.

Clean Reject is permitted only for:

- ParticipantAction after strict host schema and Action Offer admission;
- Join;
- Resume;
- AccessModeChange;
- RoleChange; and
- MembershipChangeSet when every typed component is Join, Resume, AccessModeChange, or RoleChange.

Archive, Suspend, and Depart are mandatory Core proposals. MembershipChangeSet is also mandatory when every typed component is Suspend or Depart. The pack must Apply a mandatory proposal; attempting to Reject one is PackFault. Mixed-class MembershipChangeSet never reaches the pack. A required admitted TimerFired or ExternalInput also cannot be cleanly rejected. Exact-Head, authority, integrity, policy, idempotency, and Room Commit resolution classes remain WorldStream concerns.

Declared rejection codes belong to the exact revision descriptor. An undeclared code, malformed safe detail, panic, invalid output, mandatory-Core veto, or contract-bound violation is PackFault and commits neither Transition nor new receipt.

## Host-owned timer generations

TimerRequestV1 is exactly:

~~~text
ScheduleNext {
  timer_id,
  due,
  canonical_payload
}

CancelCurrent {
  timer_id,
  expected_generation
}

RescheduleCurrent {
  timer_id,
  expected_generation,
  new_due,
  new_canonical_payload
}
~~~

The pack never assigns or predicts a timer generation. WorldStream validates the expected current witness, assigns the next monotonically nonreused generation, and records the normalized host-assigned change. One Transition may request at most one mutation per logical timer ID.

Every new due value MUST be later than the semantic effective time of the causing Stimulus. Use admitted_at for ParticipantAction, scheduled_for for TimerFired, and recorded_at for CoreProposed or ExternalInput. Equal or backward time, wrong generation, implicit replacement, conflicting requests, invalid payload/time, or overflow is PackFault. Replay repeats the same normalization from reconstructed timers without a clock or scheduler.

## View, Action Offers, and observation

ViewInputV1 contains exact Core, Activity State, complete Head, and one typed viewer:

- public/spectator Membership;
- participant Membership;
- operator Membership;
- historical Replay Membership at a reconstructed sequence;
- separately authorized post-Complete final reveal.

PackViewV1 contains one versioned canonical Activity Projection plus one canonically ordered list of ActionOfferV1 values. An Action Offer is exactly:

~~~json
{
  "domain": "worldstream/action-offer/v1",
  "action_type": "commit_move",
  "payload_schema_digest": "blake3:...",
  "eligibility_window": {
    "opens_at": "2026-08-15T12:00:00.000000Z",
    "deadline": "2026-08-15T12:00:30.000000Z"
  }
}
~~~

eligibility_window is null when no recorded window applies. Offers sort by the descriptor's action order; no alternative legality representation exists. The Host computes canonical Action Offer bytes for an exact Head and viewer. It reuses those bytes in Projection Reset, Observation Frames, Invocation Context, and Action pre-admission. An absent action type cannot reach reduce. Presence is necessary but does not guarantee acceptance of a payload-specific proposal.

ObserveInputV1 contains:

- Core and Activity before and after;
- normalized Stimulus;
- ordered Domain Events;
- exact Membership viewer;
- the exact after-view Projection and Action Offer bytes.

PackObservationV1 is one bounded canonical change value that the viewer can access. When the offers change, it carries the supplied after-view Action Offer bytes. observe returns zero or one result. If the complete authorized before/after view changes, None is PackFault; if it is unchanged, one bounded authorized notice is still allowed. A hidden Transition returns None for that viewer.

A Projection Reset calls view. Genesis creates no Observation Frame. Visibility removal, Session closure, frame sequencing and retention, Cursor movement, attach/reset barriers, and delivery remain host responsibilities.

WorldStream combines the Activity Projection, its exact ActionOfferV1 list, and authorized Core facts. The protocol Projection includes that list exactly once as the `projection.action_offers` sibling. The pack never exposes raw Activity State to any viewer, including operator Memberships. Replay and final reveal are explicit typed viewers, never an authorization bypass.

## Attention

AttentionSignalV1 names a target Membership, declared reason, priority, optional deadline, deduplication key, and the applicable exact Action Offer types. One Apply may emit at most one Attention Signal per target Membership.

The pack determines Attention canonically. The host separately checks current agent-participant eligibility, authority, policy, and integrity before creating operational Activation work. Replay reproduces Attention but performs no policy evaluation and creates no intent, offer, lease, Invocation, or Action authority.

## Bounds, faults, and containment

The descriptor's maxima cover canonical state, Domain Events, timer requests, Attention Signals, projections, observations, nesting, collections, and text bytes. The host schema-validates and canonicalizes every output before commit.

The pack produces an ActivityProjection containing only pack-owned domain information. WorldStream constructs the client-facing Projection by wrapping it with authorized Core Room facts such as Room Status and the authenticated Membership's metadata. A separate protocol Projection Envelope carries causal, operational, and delivery metadata such as complete Room Head, Room Integrity State/generation, schema, and hash. Neither layer may overwrite fields owned by another.

PackFault includes:

- callback panic where unwinding can be caught;
- malformed, noncanonical, undeclared, or oversized output;
- invalid next state, event, timer request, Attention, Projection, Action Offer, or observation;
- a mandatory Core veto;
- view/observation privacy-contract failure;
- deterministic disagreement during Recovery or Replay.

A PackFault before commit creates no Transition, timer change, Frame, Activation, or new receipt. Repeated deterministic failure on required input faults the Room; canonical hash disagreement quarantines it. Portable callbacks run in a fresh Wasmtime Store/instance with fixed fuel, memory, table, stack, byte, and concurrency limits. Engine panic/abort and aggregate process exhaustion remain process-level risks handled by pinned-engine review, process/container limits, restart tests, and upgrade gates rather than a hostile-tenancy claim.

## PackRevisionLock and startup registry

`revision_digest` is a build-computed semantic identity, not a pack-declared label and not a digest of platform-specific machine bytes. It is the digest of canonical `PackRevisionLockV1`:

- pack ID and explanatory version;
- host-contract version and canonical-codec version;
- descriptor/manifest bytes excluding the digest field;
- every schema ID and exact schema-content digest;
- deterministic static-data digests;
- pack-owned rule-source digest;
- deterministic dependency-lock digest.

Every behavior, legality, rejection, event, timer, Attention, projection, observation, or visibility change requires a new digest.

At startup the daemon constructs one immutable `PackRegistryV1` from embedded
first-party revisions plus approved local bundles. The same registry is
injected into every storage adapter and maps an exact digest to:

- the compiled executor;
- PackRevisionLockV1 and descriptor/schema bundle;
- state, configuration, Stimulus, disposition, event, timer, Attention, view, and observation codecs;
- golden-corpus digest;
- selectable_for_new_rooms;
- runnable_for_retained_rooms.

selectable_for_new_rooms implies runnable_for_retained_rooms. A digest may become non-selectable while remaining runnable. Every digest in retained Room lineage MUST remain runnable for load, advance, view, observe, Recovery, and Replay. Retaining only decoders is insufficient.

Startup rejects digest/lock/descriptor collisions and missing/corrupt approved
bytes. It never downloads, hot-loads, or compiles author source. A referenced
digest with a missing executor or codec makes the affected Room unavailable
and prevents readiness, verified restore, or transfer.

There is no in-place Room upgrade. A Room's digest and canonical Activity bytes never change except through ordinary Transitions under that same digest. A new semantic revision creates a new Room. Removing retained execution support is a future explicit compatibility break with a defined export/purge policy, never a silent migration.

Retain for the Room lineage lifetime:

- exact revision digest, revision lock, descriptor/schema bytes, codecs, executor, and golden evidence;
- Genesis, ordered Transitions, normalized Stimuli, events, timer changes, Attention, and canonical hashes;
- semantic receipt identities/tombstones and immutable referenced-resource identities needed by retained lineage.

Snapshots, current materializations, indexes, caches, telemetry, pruned delivery payloads, and retired Invocation Context bytes are replaceable or bounded. Removing every snapshot must still permit initialization and exact full Replay.

The release compatibility manifest enumerates the runtime ABI/profile and
official starter bundles. Each installation separately owns its exact approved
inventory. Backup and transfer carry the original bundle bytes for every referenced lineage. Upgrade, restore, and transfer fully Replay every retained digest. Decoding old state is not sufficient.

The existing immutable `DeploymentIdentityV1` ledger identifies the base
Runtime Distribution and its embedded Pack revisions. It is not rewritten as
local bundles are installed or revoked and is never interpreted as installed
bundle authority. The local CAS/inventory is the separate startup authority;
backup and transfer carry referenced original bundles in a separately verified
section with target-local approval.

Official portable Bundles are still local inventory. “Official” does not make
a Pack embedded or append it to `DeploymentIdentityV1`. Agent Heist is
embedded in the current base Runtime Distribution. Negotiate is installed from
its exact official `.wspack` through the offline lifecycle below.

Portable admission is serialized and bounded. Inside one admission, Wasmtime
may compile independent Component functions in parallel. This changes only the time for a cold start. The Host verifies the same bytes and uses the same deterministic settings. It admits one exact Component before the next Bundle.

## Offline operator lifecycle

Stop `worldstreamd` before changing Pack inventory. Every command uses the same reviewed configuration, owner-only data directory, and storage profile as the daemon. Each command returns a typed JSON receipt. Restart the daemon before the new inventory can affect Room creation. Use the identical
`<WORLDSTREAM_CONFIG>` for the complete lifecycle; a receipt from another
configuration is not interchangeable.

~~~bash
worldstreamctl --config <WORLDSTREAM_CONFIG> pack inspect \
  --bundle ./worldstream.negotiate.wspack

worldstreamctl --config <WORLDSTREAM_CONFIG> pack approve \
  --bundle ./worldstream.negotiate.wspack \
  --operator-id host-operator-1 \
  --decided-at 2026-08-30T12:00:00Z

worldstreamctl --config <WORLDSTREAM_CONFIG> pack install \
  --bundle ./worldstream.negotiate.wspack \
  --installed-at 2026-08-30T12:01:00Z

worldstreamctl --config <WORLDSTREAM_CONFIG> pack inventory

worldstreamctl --config <WORLDSTREAM_CONFIG> pack set-selectable \
  --bundle-digest blake3:<exact-physical-digest> \
  --selectable true

worldstreamctl --config <WORLDSTREAM_CONFIG> pack inventory

worldstreamctl --config <WORLDSTREAM_CONFIG> pack restart-readiness
~~~

`restart-readiness` reads each installed original archive again. It rebuilds the immutable registry through production Component admission. It then runs exact Genesis-to-Head Replay for every healthy Room with every retained executor. Stop the Runtime before you run it. For SQLite, the command opens the
configured `worldstream.sqlite3` through the normal production startup path.
This offline preflight can apply a supported database migration before it
captures a verified standalone snapshot for Replay. It then securely scrubs
the snapshot to bounded zero-byte markers; it does not delete their private
directories by pathname. An absent database is a verified empty deployment.
For PostgreSQL, pass the same owner-only direct-admin credential boundary used
by offline maintenance:

~~~bash
worldstreamctl --config <WORLDSTREAM_CONFIG> \
  pack restart-readiness \
  --dsn-file <POSTGRES_ADMIN_DSN_FILE>
~~~

PostgreSQL Replay runs in one repeatable-read, read-only snapshot. Both typed
receipt kinds carry `storage_profile`. Inventory and readiness receipts share
`inventory_digest`, the canonical identity of the digest-sorted Pack/version,
physical bundle, semantic revision, and install-state rows. Compare readiness
with the second inventory receipt emitted after `set-selectable`; its
`storage_profile` and `inventory_digest` must match exactly. The earlier
retained-only snapshot must have a different digest.

The readiness receipt also carries a pathless `deployment_binding`. Its inputs are the canonical data-directory identity, storage profile, and available provider deployment-lineage/storage-epoch metadata. It does not disclose raw paths or provider identity bytes. After production
Component admission and executable Replay succeed, `restart-readiness` writes
one durable startup-readiness seal binding that `deployment_binding` to the
exact `inventory_digest` and `storage_profile`. It also reports
`rooms_replayed` and `isolated_rooms_skipped`. Existing faulted or quarantined
Rooms remain isolated.

At startup, `worldstreamd` independently assembles the immutable inventory and
derives the binding from its actual configured storage target. If any portable
bundle is installed, the daemon refuses readiness unless the durable seal
matches all three values exactly. Installation, approval revocation,
selectability change, retained-bundle restore, or removal durably clears the
seal, so the operator must run `restart-readiness` again. Changes to the inventory, configuration, storage target, or retained executor invalidate readiness evidence. A canonical mismatch or healthy-Room Replay disagreement also invalidates it.

Export preserves the exact `.wspack` bytes and carries no approval:

~~~bash
worldstreamctl --config <WORLDSTREAM_CONFIG> pack export \
  --bundle-digest blake3:<exact-physical-digest> \
  --output ./exported.wspack
~~~

Revocation immediately records a local negative decision and changes installed
inventory to retained-only for the next restart. Removal has no force option. It first checks all retained Room references in the configured SQLite or PostgreSQL profile. It rejects removal of a referenced revision. PostgreSQL removal additionally requires an owner-only direct
admin DSN file through `--dsn-file`.

## Public portable Packs

The first public contract is `worldstream/component-deterministic/v1`: exactly
the five `ActivityPackV1` exports, zero imports, synchronous Wasmtime execution,
no WASI linker, fresh Store/instance per callback, original Component bytes as
authority, and fail-closed preflight/golden/privacy/resource verification.

Host Operator approval of one verified `.wspack` digest is the trust root.
Publisher signatures/provenance may add evidence but never grant authority.
Installation is offline and restart-based. There is no network registry,
marketplace, hot loading, automatic approval, name/version fallback, generic
host-import/effect surface, or force removal.

## Presentation boundary

A pack publishes typed projection schemas and semantic labels. It does not ship executable frontend code or a mutable launch URL.

An optional Activity Client is an independently released protocol peer. One
client may support several explicitly tested exact Activity Pack Revision
digests, and one Pack revision may have several clients. Client association is
deployment/integration state: it does not enter the Activity Pack Bundle,
Genesis, Authoritative Room State, Transitions, canonical hashes, or Replay.

The first product surfaces include standalone Agent Heist and Negotiate
Activity Clients plus the Pack-neutral WorldStream Inspector. The Host resolves
an exact approved Client Binding from current Membership state;
`worldstreamctl client open` launches that independent client through the
headless Controller without importing its renderer. See
[Activity Clients](activity-clients.md).

The project deliberately does not freeze:

- ViewSpec;
- a dashboard builder;
- custom widget ABI;
- pack JavaScript;
- runtime LLM-generated layout;
- automatic third-party Client Deployment or Catalog policy;
- a renderer marketplace.

## Embedded demonstration

See the [historical Heist rules](../examples/heist/rules.md) and [examples](../examples/README.md).

## Activity conformance suite

Every portable or embedded retained revision MUST pass:

- PackRevisionLock, descriptor, schema, codec, executor, and golden-corpus consistency;
- initialization determinism and no Genesis event, Attention, or Frame;
- golden Genesis/Transition lineage plus Core, Activity, aggregate, and final Activity State hashes;
- repeated reduce output equality;
- exact Core veto matrix plus participant rejection and Kernel stale-action conformance;
- all-vetoable and all-mandatory MembershipChangeSet classification, mixed-class pre-pack rejection with no receipt, and atomic final-state Role/cardinality swaps with no reflected pack ownership;
- wrong-generation, non-forward, duplicate/conflicting timer-request faults;
- maximum-state, collection, nesting, text, and output bounds;
- Action Offer byte parity across view, observation, reset, Invocation Context, and host pre-admission;
- projection/observation schema validation and zero-or-one viewer output;
- randomized cross-participant privacy/noninterference;
- completed-reveal authorization;
- activation-reason declaration and deduplication;
- callback panic and malformed-output containment before commit;
- crash recovery from an older paired Core-and-Activity snapshot and from Genesis with every snapshot deleted;
- Replay without external I/O, policy evaluation, scheduler, or Activation creation;
- present-plus-historical Replay authorization across suspend/depart/rejoin and Role changes;
- retained non-selectable digest remains fully runnable through load, advance, view, and Replay;
- absence of floating-point authoritative values;
- no core changes specific to the pack.

## Pack Author path

The first public SDK is TypeScript through release-pinned
`@worldstream/pack-sdk` and `@worldstream/pack-cli`. One code-first project
supports scaffold, strict check, tests, build, inspect, and production-host
proof. Optional first-release prompt assistance emits the same reviewable
project and gains no install or Room authority. Authors never hand-write WIT,
schemas, digests, revision locks, Component bytes, or conformance evidence.

The offline command `worldstream-pack new <directory>` remains canonical.
`worldstream-pack create <directory> --prompt "<description>"` may call a
configured OpenAI-compatible endpoint, but it accepts only a closed blueprint
selecting the fixed starter plus its display name and description. WorldStream
renders every file locally below one private staging root and returns
`review_required`; only the separate
`worldstream-pack create <directory> --confirm <review-id>` command promotes
the exact reviewed bytes. Both routes then use the identical `check`, `test`,
`build`, `inspect`, and `prove` pipeline.

`prove` compiles the Component once and submits that draft bundle to the
production verifier, Component Host, and Core registry. When Core returns the
authoritative retained-transcript digest for that exact semantic revision, the
CLI may finalize only `golden-corpus.json`, `conformance.json`, and the bundle
manifest before submitting the exact same Component and revision a second
time. Every other member is byte-compared across finalization; revision drift,
a repeated finalization response, or any non-transcript admission failure is
fatal.

Prompt assistance cannot accept provider-authored source, paths, dependencies, shell commands, or executable rules. It cannot follow symlinks, escape either root, or install packages. It cannot build, approve, or install a bundle, or contact the daemon.
The CLI discards provider response bodies, endpoint, model, key, and prompt transcript. These values MUST NOT enter project or bundle bytes, proof evidence, command receipts, or normal diagnostic logs. External endpoints require HTTPS;
unencrypted HTTP is permitted only for a loopback provider.

Negotiate MUST dogfood this exact path and match a native first-party oracle.
Go and additional source languages are deferred; Wasm remains the neutral
deployment target rather than the authoring language.

# Security and Trust Model

## Status

WorldStream is an experimental implementation for local or operator-controlled installations. There is no supported production release.

This document describes its trust boundaries. It does not establish protection for a public multi-tenant service or hostile code.

## Trust statement

Trusted:

- the host operator;
- the installed WorldStream binary, compatibility manifest, and explicitly selected configuration;
- embedded first-party revisions and exact Host Operator approval records;
- the operating system, selected durable storage profile, and its operator-controlled credentials/backup boundary.

Untrusted:

- remote human clients;
- remote agent clients and runners;
- every submitted action and acknowledgement;
- free text produced by humans or models;
- v0.2 artifact bytes and metadata;
- browser state and URLs;
- remote network transport and any hosted PostgreSQL control-plane API;
- network timing, duplication, reordering across reconnect, and disconnection.
- portable Activity Pack code and its self-reported conformance claims.

Portable Packs are operator-approved untrusted application code executed by a
capability-denying Wasmtime host. Approval authorizes exact bytes; it does not
make them intrinsically trusted. WorldStream claims zero imported capabilities,
deterministic limits, and fail-closed Room semantics—not immunity from engine
defects or hostile multi-tenant process isolation. Embedded Rust revisions are
trusted internal bridges/oracles and retain ordinary same-process risk.

## Protected assets

- immutable Genesis/Transition lineage, Authoritative Room State, and all Core/Activity/aggregate hashes;
- Room Integrity State/generation and append-only incident/repair audit;
- participant-private projections and observation frames;
- private clues, offers, commitments, evidence assignments, and draft work;
- capability tokens and token hashes;
- activation context and lease ownership;
- action idempotency and stable results;
- selected database (including SQLite WAL/shared-memory files), Storage Epoch/lineage, paired snapshots, receipts, transfer bundles, and backups;
- v0.2 content-addressed evidence artifacts;
- operator controls and replay authorization;
- availability of bounded room and session resources.

WorldStream does not accept or protect model-provider passwords, subscription OAuth caches, API keys, wallet keys, or private chain-of-thought because they must never enter the server.

## Threat actors

### Malicious or curious participant

May try to:

- read another participant's private projection;
- guess room/member/artifact identifiers;
- use replay to reveal hidden state;
- submit an action for another role;
- exploit stale-state races;
- reuse IDs with different payloads;
- inject markup or instructions into another agent/UI;
- flood a room or connection.

This applies equally to humans and agents.

### Compromised agent runner

May:

- claim activations it is not authorized to handle;
- replay or race claim leases;
- submit malformed or malicious model output;
- lie about invocation completion;
- retain data it legitimately received;
- attempt to make the server expose provider credentials.

WorldStream limits authorization and authoritative consequences. It cannot prevent an authorized runner from retaining data already disclosed to it.

### Hostile artifact author

May upload:

- oversized or endless streams;
- path-traversal filenames;
- MIME-spoofed executable content;
- active SVG/HTML;
- decompression bombs;
- malicious prompt instructions;
- duplicate/digest-conflicting bytes.

The server stores immutable bounded blobs but does not execute or unpack them.

### Network attacker

May eavesdrop or alter traffic if TLS is absent, steal bearer capabilities, replay envelopes, hold sockets open, or exploit parser limits.

Loopback is the default. Remote use requires TLS termination and careful capability handling.

### Buggy pack or server

May leak state through a projection, produce nondeterministic replay, create unbounded output, mishandle a timer, or acknowledge before commit. Tests and fail-closed behavior are the primary controls.

## Data classification

| Class | Examples | Default handling |
|---|---|---|
| Public | Heist public plan, public Outcome, closed-case published board | Public Projection only |
| Member-private | Clue, offer, sealed move, assigned evidence, draft fact | Exact member audience |
| Operator-private | Diagnostics, token metadata, fault details | Local scoped operator access |
| Secret | Bearer token input, TLS private key | Never logged; token only hashed in DB |
| Prohibited | Provider credentials, consumer login cache, chain-of-thought | Reject/do not collect |
| Artifact | Evidence bytes, metadata | Digest-addressed, room-authorized, size-limited |

Activity Packs must classify every projection field. The absence of a label does not make data public.

## Security invariants

1. Authenticate a principal and capability before room attachment.
2. Authorize every room, membership, action, replay, activation, and artifact operation.
3. Do not trust a client-supplied Principal kind, Role, Room sequence, Action Offer, or audience; Action Offers come only from the exact pack view.
4. Never expose Canonical Activity State or Core Room State directly.
5. Construct public, participant, and operator Activity Projections through separate typed paths, then wrap them in the authorized core Projection envelope.
6. Authorization occurs before observation persistence, artifact lookup, indexing, filtering, or rendering.
7. Accepted actions are committed before acknowledgement.
8. Same room/member/action ID cannot mutate twice.
9. Same ID with different canonical payload is rejected.
10. All queues and payloads are bounded.
11. Activation offer metadata contains no private projection.
12. Private activation context is returned only after an authorized lease claim.
13. Replay requires present authorization and reconstructed historical Membership/Access/Role authorization; it cannot bypass pack-defined reveal policy.
14. Logs and metrics exclude secrets and private payloads by default.
15. Artifact paths derive only from verified digests, never user filenames.
16. Pack code receives no host I/O capability through its interface.
17. Canonical hash or deterministic Replay disagreement quarantines the Room; an intact Head that cannot safely advance faults it.
18. Transient admission/runtime errors do not consume an action ID.
19. An expired activation claim cannot operate on a later lease generation.
20. Immutable Genesis plus Transitions remains sufficient after all paired snapshots and current materializations are removed.
21. Every new Existing Advance or durable disposition fences on healthy Room Integrity State plus an unchanged generation; Create instead initializes healthy generation 1 without a pre-existing integrity witness.
22. Only a successful generation-fenced verifier may restore healthy, and repair never rewrites canonical lineage.
23. Operator Membership never receives raw Activity State; every operator result is an explicit bounded pack view.
24. Every retained digest resolves to its exact executor/codecs; a newer revision never interprets old lineage.

## Authentication and capabilities

v0.1 uses development-grade scoped bearer capabilities:

- generate at least 256 random bits;
- let worldstreamctl generate and display the plaintext locally for administrative capability creation;
- send only the protocol-defined derived token hash to the server;
- persist a memory-hard or strong keyed/token hash, never plaintext;
- bind scopes to principal and optionally room/member;
- support expiration and revocation;
- compare tokens in constant time;
- never place tokens in URL query strings;
- redact Authorization and cookie headers from logs.

Example scopes:

- room:attach;
- room:act;
- room:observe_public;
- room:observe_member;
- room:replay;
- activation:offer_receive;
- activation:claim;
- activation:complete;
- artifact:upload;
- artifact:read;
- operator:room_admin;
- operator:backup.

Activation-control scopes never imply room:act or observation acknowledgement authority. A Runner that acts for an Agent Participant must also present separate authority bound to that Principal and Membership, following [ADR 0003](adr/0003-separate-activation-and-action-authority.md).

Role authorization remains pack-defined after the Room Kernel verifies identity and declared action kind.

The frozen releases do not implement SSO, OAuth account linking, organization tenancy, SCIM, or fine-grained enterprise policy.

## Session and protocol defenses

### Input parsing

- strict JSON and UTF-8;
- reject duplicate keys and unknown fields;
- no floating-point authoritative values;
- maximum WebSocket message, action, frame, state, and artifact sizes;
- bounded nesting depth, strings, arrays, and object entries;
- request read timeout;
- schema validation before pack entry.

### Rate and resource limits

Limit by:

- source IP for unauthenticated attempts;
- principal and capability;
- session;
- membership;
- room mailbox;
- activation claim attempts;
- artifact bytes and temp disk;
- operator endpoint.

Backpressure is explicit. The server rejects room_busy or rate_limited and disconnects a slow consumer instead of allocating unbounded memory.

The reference gateway enforces the architecture's fixed token-bucket table in
one bounded in-memory limiter shared by HTTP and WebSocket traffic. Socket peer
addresses, not `Forwarded` or `X-Forwarded-For`, select IP buckets; missing peer
metadata shares one fail-closed bucket. Principal keys come from the selected
backend's successful authority authentication; capability keys derive from the
Core bearer's stable token hash, never its wire value. Session keys come from
the server-generated connection Session ID, Membership keys combine Room and
Member IDs, and Room keys are independent. Activation claim/lease keys combine
verified Principal, capability, and a closed operation class rather than a
client-supplied activation ID. Operator keys combine verified Principal, capability, and a closed endpoint class. A caller cannot create arbitrary keys or impose a shared global endpoint throttle. HTTP requests are
IP-limited before routing and authenticated operations are limited again before
backend work. Runner-Capability issuance consumes one operator admission rather
than one admission per target. Every WebSocket message is limited before
decoding/dispatch, with valid targeted envelopes receiving the additional
Membership and Room checks.

The limiter stores only process-keyed BLAKE3 fingerprints and bounds the live
key count per scope. A target requires a verified Principal and an atomic association that expires when idle. One Principal can have at most 256 distinct live targets. The process can have at most 65,536 associations. Quota rejection
refreshes existing presented associations, preventing an attacker from aging
out ownership while preserving global target buckets. Ten-minute idle expiry
reclaims key and association capacity.

WebSocket limits are 256 pending permits, 4,096 active permits, and 64 active permits per Principal. The browser-ticket deadline is 15 seconds. The first-hello deadline is 10 seconds. After welcome, the inbound-idle deadline is 90 seconds. Pending-to-active conversion and release are atomic RAII operations.
Exhausted buckets, target or connection capacity exhaustion, invalid key
material, poisoned state, and clock regression all fail closed with a generic
retryable `rate_limited` response. A rejected message reaches no semantic
backend operation, creates no receipt, and exposes neither the exhausted
identity nor its private target. Prometheus labels are a closed scope
vocabulary (`ip`, `principal`, `capability`, `session`, `membership`, `room`,
`activation`, `operator`, and the fixed failure classes); bearer values and
entity IDs never become labels.

### ID and replay confusion

- room/member/action forms the action deduplication boundary;
- Action Semantic Receipt stores the Canonical Request Hash;
- activation/claim IDs are separately scoped;
- Cursor cannot move backward through acknowledgement or beyond the Membership frame head; pruning cannot move it;
- a Session synchronization token is opaque, single-use, bound to its Session/barrier, and never satisfied by another Session's acknowledgement;
- a bearer token for one member cannot attach as another;
- server-generated transition and frame sequences are never accepted from client assertions.

### Stale and transient actions

Each action includes based_on_room_seq. The Room Kernel requires exact equality with the current room head before pack reduction in the frozen releases. A deterministic stale rejection can safely reveal only a current sequence and exact current Action Offers.

Unauthenticated, forbidden, malformed, room_busy, rate_limited, storage_unavailable, and activity/runtime-fault responses create no action receipt. A caller may retry the identical canonical action with the same action ID. Deterministic admitted domain rejections are durably receipted and consume that ID.

## Projection and privacy isolation

The largest application-layer risk is accidental cross-participant leakage.

Required design:

- Canonical Activity State and Core Room State use types unavailable to gateway serialization;
- public, participant, operator-membership, and final-reveal projections are different Rust types or wrappers;
- private frames name exact member audiences;
- public payloads are constructed separately rather than redacting private payloads, then copied into each authorized membership's one durable stream;
- another member ID in a URL/body never changes authenticated viewer identity;
- frame caches include audience in every key;
- projection reset uses the same authorization path as live projection;
- Replay first authenticates/authorizes the present requester, then reconstructs Membership existence, Standing, Access Mode, and Role at sequence N to select the historical viewer;
- a Membership absent at N receives no participant/private view, and a later Role or replacement Member ID inherits no earlier private data;
- spectator/operator history and final reveal require explicit present projection policy.

Required tests:

- randomized role and membership changes;
- join after private events;
- leave/rejoin;
- cursor catch-up and reset;
- replay during active and closed rooms;
- public UI DOM inspection;
- server logs and error details;
- artifact-reference enumeration.

Noninterference test principle:

> Changing a hidden field for participant B must not change participant A's serialized projection unless the pack explicitly publishes a consequence visible to A.

## Room integrity, serving, and repair

Room Integrity State is durable operational security state outside Core Room State, Authoritative Room State, Room sequence, Replay state, and canonical hashes:

- healthy permits canonical advance;
- faulted means the last canonical Head verifies but the runtime cannot safely advance it;
- quarantined means canonical integrity cannot be established.

Every integrity change increments a monotonic generation and appends an incident/repair record. Every new Existing Advance or durable disposition conditionally matches both healthy and the generation captured during preparation. Create atomically initializes `healthy` at generation `1` but has no pre-existing integrity witness. A lost Existing fence writes no Transition, sequence, or receipt. No canonical participant or administrative mutation is allowed while faulted or quarantined; operational capability revocation remains immediate.

A faulted Room may expose only its last verified authorized Projection, retained Frame Catch-up, and verified Replay with explicit integrity metadata. A quarantined Room exposes no normal Projection, Catch-up, or claimed-current Replay. Authenticated host-operator diagnostics, raw export, restore, and verification remain available, with safe bounded details that do not disclose private state to a Room Member.

An operator can request repair but cannot mark a Room healthy. Only a successful verifier conditioned on the current generation may do so. The verifier may rebuild materializations/caches, reinstall the exact retained Pack, or restore exact canonical bytes from a verified backup. It cannot edit, skip, reorder, synthesize, or replace Genesis/Transitions.

## Agent runner and activation security

Membership and Runner authorization are separate. A Runner has a server-issued ID, and its capability binds that ID plus the Agent Participant Memberships it may claim.

Activation offer contains only:

- activation ID;
- room/member identifiers already authorized to the runner;
- pack ID;
- reason code;
- priority, deadline, and lease duration.

It MUST NOT contain:

- private observations;
- artifact bytes or secret URLs;
- model/provider credentials;
- other participants' state;
- prompts or chain-of-thought.

After an atomic successful claim, the server returns only the exact committed Invocation Context for the target Agent Participant Membership. Context contains the authorized Projection/hash, complete Head and witnesses, Action Offers, and Artifact references. It includes exactly one retained-frame range or Projection Reset baseline.

Lease defenses:

- one live lease per Membership across all intents;
- independent idempotent operation IDs/request hashes for claim, renew, release, and complete;
- monotonically increasing lease generation;
- bounded maximum lease;
- server time controls expiry;
- renewal, release, and completion require the authenticated runner, current claim ID, current generation, and unexpired lease;
- stale claim completion is rejected;
- repeated delivery does not create another logical activation.

Archive and affected Membership/Access/Role changes cancel and generation-fence pending/leased intents. Capability revocation takes effect immediately. Runtime recovery state Loading, CatchingUp, Passivating, or Inactive, or Room Integrity State faulted or quarantined, makes pending intents unclaimable. A backward-clock anomaly fences current leases before work can resume.

Replay verifies only canonical Attention and recorded decision evidence. It does not evaluate policy, return live Invocation Context, or create or offer an intent. It does not grant a lease or contact a Runner.

A runner-reported handled status is operational. Only accepted room actions have authoritative effect.

The server cannot prove that a claimed runner invoked a model, used a particular model, or deleted received context. Do not claim otherwise.

## Prompt injection and hostile text

WorldStream treats all human, agent, and artifact text as data. It cannot make an LLM immune to prompt injection.

Controls:

- system/SDK context separates server-authored fields, participant text, and artifact text;
- typed actions limit authoritative consequences;
- free text has bounded length and no implied tool authority;
- artifact text does not create actions or capabilities;
- private provider credentials stay with the runner;
- reference activities have no external side-effect actions;
- deterministic rules, not an LLM judge, decide outcomes;
- UI escapes and labels untrusted content and provenance.

Runner authors should treat room text as untrusted input and enforce their own tool policies.

WorldStream does not store private chain-of-thought as a debugging or reputation signal.

## Activity Pack security

### Portable and embedded execution model

- public Packs are exact `.wspack` bundles approved by digest and loaded only at startup;
- PackRevisionLock semantic digest is pinned per Room;
- one immutable startup registry maps embedded and approved revisions to exact executor, descriptor/schemas, codecs, golden digest, and selectable/runnable status;
- every retained digest remains runnable even when no longer selectable;
- ActivityPackV1 exposes exactly descriptor, initialize, reduce, view, and observe and no I/O, clock, scheduler, Activation, Session, delivery, telemetry, or artifact-byte handle;
- Canonical Activity State, events, timer requests, Attention, Projection, Action Offer, observation, nesting, collection, and text limits apply;
- a public Component has exactly five exports, zero imports, no WASI linker, and a fresh Store/instance per callback;
- original Component bytes are authoritative; AOT cache is host-owned and disposable;
- fixed revision-bound fuel, memory, table, stack, byte, and concurrency limits apply;
- traps and panics are classified at the host boundary where possible;
- malformed output, mandatory-Core veto, privacy/view failure, or bound violation commits nothing;
- repeated deterministic required-input fault faults the Room; hash/Replay disagreement quarantines it;
- replay tests detect nondeterminism;
- supply-chain review covers pack dependencies.

The zero-import profile denies guest clock, entropy, filesystem, network,
environment, process, storage, scheduler, model, credential, thread, and host
callback access. Aggregate process exhaustion and engine aborts remain
process-level risks.

### Deliberately deferred capabilities

The first profile has no generic helper imports, WASI, host effects, hot loading,
network registry, automatic approval, or executable renderer code.
Signatures/provenance are optional evidence and never replace exact-digest
approval or host verification.

## Timer security

- timer IDs and generations are room-scoped;
- schedule/cancel changes commit with their causing transition;
- timer payloads are size/schema validated;
- firing is deduplicated under a database constraint;
- scheduler lag is measured;
- overdue timer storms are rate-limited per room;
- a timer cannot name an arbitrary callback URL;
- replay reuses recorded TimerFired input without scheduling anything.

## v0.2 artifact security

### Upload

- authorize before accepting bytes;
- enforce Content-Length when present and streaming byte limit always;
- write only to a generated temp path;
- require temp and final CAS path on the same validated local filesystem;
- set owner-only permissions;
- calculate BLAKE3 while streaming;
- fsync the file, atomically rename to digest path, and fsync the target parent directory before any database reference can commit;
- persist an owner-scoped expiring staged-upload record and require a later typed action to link it;
- verify any client-provided digest;
- use quota-limited temp storage;
- clean abandoned temp files safely;
- garbage-collect a durable CAS orphan only after a grace period and only when no artifact, room reference, or staged upload names it.

### Paths

- final paths contain only server-generated hexadecimal digest segments;
- reject path separators, dot segments, symlinks, and user-selected absolute paths;
- open files without following symlinks where supported;
- never use original filename for filesystem lookup.

### Content

- detect a conservative media type from bytes where practical;
- do not unzip, execute, convert, or index archives;
- block active HTML/SVG preview;
- force attachment for unknown or risky media;
- escape text preview;
- cap preview bytes;
- make external agents fetch bytes under their own authorization rather than embedding blobs in observations.

### Authorization and indexing

- authorize room and viewer before resolving a digest;
- visibility is pack state, not artifact metadata alone;
- cache keys include room and audience;
- any FTS index is partitioned/filtered by authorized evidence set before query;
- result counts, snippets, and timing must not expose hidden documents.

### Backup consistency

- acquire an artifact-GC/deletion lease;
- create a backend-native SQLite or PostgreSQL backup and immutable backup ID;
- record Storage Epoch, engine/schema/migration/codec/pack-executor identities and an exact digest manifest;
- copy exactly those immutable artifacts;
- verify every referenced size and digest;
- durably finalize backup data, manifest, files, and destination metadata before reporting success;
- report missing/unreferenced files;
- restore into an empty isolated target and run backend checks plus the full WorldStream semantic verifier before readiness.

## UI security

- React escapes text by default; do not use unsafe HTML injection;
- if Markdown is added, disable raw HTML and use a strict sanitizer;
- allowlist URL schemes;
- validate WebSocket Origin;
- use a restrictive Content Security Policy;
- do not permit pack-supplied scripts, CSS, iframe URLs, or network endpoints;
- prevent clickjacking where remote operation is enabled;
- distinguish public, participant, and operator bundles/routes;
- disable mutating controls in replay;
- reauthorize every server request regardless of UI state.

## Persistence, recovery, and filesystem security

- the backend is selected once at startup; loss of PostgreSQL fails readiness and mutation rather than falling back to SQLite or memory;
- SQLite, WAL, and shared-memory files remain together on a validated supported local filesystem, never network/UNC, shared, symlink/reparse, or container-overlay storage;
- POSIX data and secret paths are owner-only; Windows DACLs permit only the service identity/owner, SYSTEM, and administrators and reject broad write;
- startup validates path type, permissions/ACL, exclusive SQLite lock where applicable, free space, durable filesystem operations, exact engine/manifest/schema/migrations, Storage Epoch, and global integrity;
- remote PostgreSQL requires TLS, `synchronous_commit=on`, a least-privilege daemon role, and no provider API, extension, session-state, named-prepared-statement, or replica correctness dependency;
- immutable canonical Genesis and its three initial state hashes are verified before recovery;
- snapshots are paired Core-and-Activity postcommit caches that bind the complete Head and verify both canonical values, both component hashes, aggregate hash, and applicable lineage hash; a failed pair falls back to an older verified pair or Genesis;
- current Room/Membership/Activity rows are verified serving materializations, not an independent authority;
- deleting all paired snapshots and current materializations still permits recovery from Genesis plus Transitions;
- every SQLite connection enables foreign-key enforcement; startup/restore runs backend-native integrity and referential checks;
- missing Transitions or canonical hash mismatch quarantine a Room;
- no acknowledged action depends on an unflushed in-memory state;
- disk-full and I/O errors make readiness fail and mutations stop;
- logs go to stdout and never into artifact paths;
- temporary cleanup never traverses outside the configured tmp directory.

At-rest encryption is an operator disk or PostgreSQL provider/service concern in frozen releases; WorldStream adds no application-layer encryption.

## Logging and telemetry

Safe default log fields:

- request/session correlation ID;
- room, membership, action, activation, and transition IDs;
- message type and result code;
- sizes, durations, queue depths, error codes;
- server/protocol/Activity Pack revision.

Unsafe default fields:

- bearer tokens or token hashes;
- action/observation payload bodies;
- private clues, evidence, drafts, commitments;
- artifact content or signed download URL;
- model prompts, outputs, provider metadata, or chain-of-thought.
- database/admin DSNs, secret-file contents, and exporter credentials.

Metrics use low-cardinality counts and sizes, never entity IDs or raw participant text. After commit, bounded nonblocking queues carry JSON logs, Prometheus metrics, W3C trace correlation, and optional OpenTelemetry/OTLP export. Exporters do not participate in admission, reduction, commit, Replay, Room Integrity State, or readiness. Overflow drops telemetry with a metric/rate-limited warning, and shutdown flush is bounded to three seconds.

## Required security tests

Before the first public portable-Pack release:

- bundle traversal/link/duplicate/member-tamper and atomic-install crash tests;
- exact import/export/version preflight, forbidden-feature, Component-size, original-byte restart, and cache-misbinding tests;
- fuel, memory, table, stack, byte-boundary, compile-concurrency, and diagnostic-redaction tests;
- missing/corrupt retained bundle readiness plus backup/restore/transfer and engine-upgrade Replay tests;
- Negotiate exact-approval mutation, signer/mandate/head/deadline, privacy, and dual-evidence verification tests;

- PackRevisionLock/digest collision and exact-executor/codec retention tests;
- Action Offer byte parity across Projection, reset, Frame, Invocation Context, and admission;
- Core veto matrix plus TimerFired-field and pack-assigned-generation rejection;
- callback panic, malformed output, privacy/view failure, and every declared bound with no partial commit;
- full three-seat/six-phase majority/scoring/timer/Attention golden corpus;
- protocol fuzzing and duplicate-key cases;
- oversized/nested payload rejection;
- authentication/scope matrix;
- action ID replay and conflicting payload;
- transient action error followed by same-ID successful retry;
- stale-action race;
- randomized private-projection noninterference;
- public/replay/catch-up leakage;
- no-Genesis/zero-frame hidden Transition and zero-or-one coalescing checks;
- first/pruned attach Reset plus Session-token barrier isolation across two Sessions;
- bounded mailbox and slow-consumer soak;
- activation offer/claim authorization and lease races;
- same Activation operation ID with altered request or Runner identity is rejected;
- expired claim generation attempting to complete a newer lease;
- archive/Membership/authority/policy/backward-clock generation fencing;
- exact Invocation Context contains one retained-or-reset branch and does not advance Cursor;
- Replay creates no intent, context, lease, offer, or Runner contact;
- catch-up/live handoff transition with no missing frame;
- mutating HTTP lost-response idempotency;
- log/token redaction;
- stored text XSS;
- forced process termination and SQLite recovery;
- recovery with every paired snapshot and current materialization deleted;
- corrupt paired-snapshot fallback, Genesis verification, foreign-key check, three-state-hash and Transition-hash failure;
- atomic multi-Membership Role swap and terminal departed/new-ID rejoin privacy;
- present-plus-historical Replay authorization across Role/Access/lifecycle changes;
- faulted versus quarantined serving matrix;
- integrity-generation commit/repair races and verifier-only healthy restoration;
- cache/materialization repair proving byte-identical Genesis/Transitions before and after;
- native PostgreSQL 17 direct and transaction-pooler conformance, least-privilege/TLS failure, connection replacement, and forced termination around COMMIT;
- every prior forward migration on both backends, SQLite pre-migration backup/restore, and PostgreSQL offline-admin migration/isolated restore;
- resumable two-phase SQLite-to-PostgreSQL transfer, abort before retirement, Storage Epoch stale-source rejection, exact byte parity, timer/Activation fencing, and no reverse/live/dual-write path;
- full semantic verifier coverage of every healthy Room plus preservation/isolation of a pre-existing faulted or quarantined Room;
- native Linux, native Windows ACL/filesystem/SQLite/PostgreSQL/recovery, Linux/amd64 OCI persistence, and macOS source-quickstart evidence;
- recovery with every paired snapshot/current materialization deleted on both profiles;
- corrupt paired-snapshot fallback, Genesis verification, referential checks, all three state hashes, and Transition-hash failure on both profiles.

Before any future Investigation artifact release:

- artifact path traversal and symlink cases;
- byte-limit and temp-space exhaustion;
- MIME spoofing and active-content preview;
- digest mismatch and duplicate upload;
- crash between CAS rename, parent-directory fsync, staged-upload receipt, and authoritative link;
- staged-upload expiry and orphan reconciliation never delete linked or live-staged bytes;
- authorization before artifact access/search;
- evidence-version visibility and invalidation;
- malicious prompt content treated as data;
- database/artifact backup consistency.

## Deferred production hardening

A public hosted service would additionally need:

- real tenant isolation;
- SSO/OIDC, organization policy, and administrator separation;
- TLS automation and secret manager integration;
- database encryption and key rotation;
- DDoS/WAF and abuse controls;
- moderation and content policy;
- formal pack sandbox;
- vulnerability response policy;
- audit-log retention policy;
- regional privacy and compliance review;
- HA, provider failover services, and an uptime model.

These are not reasons to fake production readiness in the hobby-project releases.

## Vulnerability reporting

Before public release, add SECURITY.md with a private reporting channel, supported versions, expected response window, and a clear request not to publish participant-private proof data.

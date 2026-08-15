# Context, State, and Memory

## Status

This document explains supporting data architecture. WorldStream is not positioned as a generic context layer, memory server, RAG system, or vector database.

The frozen rule is:

> WorldStream remembers the shared room exactly and gives each invocation a bounded authorized projection of that room.

Agent identity can be durable while every model invocation is ephemeral. The room persists; the model's hidden context does not.

## The core distinction

These concepts must not be collapsed:

### Core Room State

The versioned WorldStream-owned value containing exactly Room Status and the canonically sorted semantic Membership map. Each Membership carries immutable Member/Principal identity and Principal kind plus standing, Access Mode, and current Role; the Room Head and every operational field are outside this value.

### Activity State

Activity Pack-owned current domain facts such as phase, clues, evidence versions, claims, deadlines, and Outcome. Exact Action Offers are derived by the pack view from these facts and the current Membership.

### Authoritative Room State

The accepted current truth of the Room: Core Room State together with Activity State. Clients receive authorized Projections, never this aggregate directly.

### Room Integrity State

Durable operational `healthy | faulted | quarantined` state with a monotonic generation and separate incident/repair audit. It is outside Authoritative Room State, Room order, Replay state, and canonical hashes.

### Canonical history

The immutable Genesis followed by ordered Transitions, including each accepted Stimulus, deterministic outputs, and Core/Activity/aggregate hashes, from which both state components and lineage can be reconstructed and verified.

### Projection

The current subset of Authoritative Room State that one Membership is authorized to see.

### Observation stream

The bounded Membership-specific changes after a Cursor. Its frame head, retained floor, and Cursor are distinct; Genesis emits no frame and each later Transition emits at most one coalesced frame per Membership.

### Artifact

An immutable external blob, introduced for Investigation Room, referenced by content digest and structured metadata.

### Invocation context

The exact temporary payload committed with one granted claim: complete Head and witnesses, current authorized Projection and Action Offers, cause/deadline/limits/Artifact references, and exactly one retained-frames or Projection-Reset branch.

### Agent-private memory

Anything a runner or agent chooses to keep outside WorldStream: preferences, summaries, embeddings, model sessions, private notes, or tool history. The server does not own it.

## Why canonical history or Markdown is not the prompt

Canonical transition history is good for truth and recovery but poor model input:

- it grows without bound;
- transitions and their domain events include superseded facts and operational detail;
- another participant may not be authorized to see them;
- many changes are irrelevant to the next action;
- a raw transcript obscures current Action Offers and deadlines.

A Markdown file is useful as an Artifact or human-readable export, but it is not an Authoritative Room State format. Unstructured text makes deterministic invalidation, precise authorization, and Replay harder.

An embedding index can retrieve semantically similar text, but similarity is not authority, freshness, visibility, or causal dependency. It is therefore absent from v0.1 and v0.2.

## Frozen data planes

~~~mermaid
flowchart TD
    H["Canonical History: Genesis plus Transitions"] --> S["Verified current Core plus Activity materialization"]
    H --> SN["Paired postcommit Core plus Activity snapshots"]
    I["Operational Room Integrity State plus generation"] -. "fences serving and Existing writes" .-> S
    S --> P["Pack-authorized projections"]
    H --> O["Membership observation frames"]
    P --> C["Invocation context"]
    O --> C
    A["Immutable artifacts and metadata"] --> C
    S --> A
    H --> R["Read-only replay"]
~~~

### 1. Canonical transition history

Purpose:

- source of accepted change;
- idempotent action result;
- recovery and audit;
- deterministic replay;
- causal explanation.

Properties:

- append-only per room;
- compact typed canonical JSON;
- exact Core schema and Activity Pack revision lock/executor identity plus Core, Activity, aggregate, and lineage hashes;
- no token fragments, connection heartbeats, debug traces, or chain-of-thought;
- retained for the room lifetime in frozen releases.

The history answers what happened, not what an agent should read wholesale.

### 2. Current Authoritative Room State

Purpose:

- current core and activity truth;
- action validation;
- timer and dependency rules;
- source for authorized projections.

The active Room actor keeps verified canonical Core and Activity State plus the complete Room Head. Current Room/Membership/Activity rows and actor memory are materializations, not a second authority. Immutable Genesis plus Transitions reconstruct the aggregate; a verified paired snapshot may accelerate both components at one sequence.

Canonical Activity State is pack-defined and bounded to two MiB by default. Large evidence bytes do not belong in it; Activity State holds digests and metadata.

### 3. Authorized projections

Purpose:

- current authorized situation;
- current Action Offers;
- deadlines and obligations;
- bounded model- and UI-ready structure.

A projection is regenerated from current state and viewer identity. It is not a permanently growing transcript.

The pack, not a generic LLM, decides what each role may see. Projection code is deterministic and covered by privacy tests.

### 4. Membership observation frames

Purpose:

- live incremental delivery;
- exact reconnect from a membership cursor;
- activation-relevant change range.

Frames are materialized by explicit audience, zero or one per Transition and Membership. They may be pruned after acknowledgement plus a retention window because an authorized current Projection can reset an old Cursor. Pruning changes the retained floor, never the Cursor or frame head.

Pruning a frame does not delete canonical history or current state.

### 5. Immutable artifacts

Artifact storage appears only in v0.2 for Investigation evidence.

Artifact bytes:

- are addressed by BLAKE3 digest;
- live in the local content-addressed filesystem;
- are immutable and deduplicated;
- have generic size, media type, visibility, and room-reference metadata in SQLite;
- are never executed, embedded, or summarized by WorldStream.

Activity state records exact evidence version, provenance label, assignment, visibility, supersedes relation, and downstream fact IDs.

### 6. Derived indexes and caches

Any future full-text index, host-operator filter, cached projection, or metrics aggregation is:

- derived;
- authorization-filtered before access;
- rebuildable from canonical data;
- never used to decide truth;
- never included in Core, Activity, aggregate, or Transition hashes.

FTS5 may be evaluated after the structured Investigation fixture works. It is not a release requirement. Vector indexes and semantic summary stores are explicitly out of scope.

### 7. Durable operational integrity

Room Integrity State and its monotonic generation survive restart but are not canonical Room truth. Healthy permits advance. Faulted means the last Head verifies but the runtime cannot advance safely and may serve only last-verified authorized data with an integrity envelope. Quarantined means canonical integrity cannot be established and permits only authenticated host-operator diagnostics, raw export, restore, and verification.

Every new Existing Advance or durable disposition matches healthy plus an unchanged generation. Create initializes operational Room Integrity State to `healthy` at generation `1` without a pre-existing integrity witness. An operator may request repair; only a generation-fenced verifier may restore healthy after rebuilding materializations/caches, reinstalling the exact pack, or restoring exact canonical bytes. Repair never edits, skips, or replaces Genesis/Transitions.

### 8. Ephemeral operational data

Connection presence, heartbeat timing, in-memory send queues, temporary upload progress, and metrics samples are operational. They do not consume canonical room sequence.

Runner availability is also operational. A durable activation remains pending even if no runner connection exists.

## Synchronization and Invocation-context assembly

A Room attach returns only its Session synchronization contract: the captured Room/frame barrier, retained frames or a Projection Reset, and the Session-specific token. It does not create an Invocation Context or grant Runner authority.

WorldStream does not let an LLM invent a context query. Only after a successful Activation claim does it return the exact bounded Invocation Context committed with that grant.

Order of precedence:

1. Identity and cause
   - Room, Membership, Role, and Principal kind;
   - activation ID and typed reason;
   - cause room sequence and complete exact Room Head.
2. Current truth
   - pack-authorized current projection;
   - current phase and state version;
   - current Action Offers, byte-identical to the pack view used for admission.
3. Time constraints
   - personal and room deadlines;
   - activation lease expiry;
   - action based-on sequence.
4. Relevant changes
   - exactly one retained Observation Frame range after the Membership Cursor;
   - or an explicit Projection Reset baseline if that range is unavailable.
5. Evidence
   - exact authorized artifact references selected by pack state;
   - digest, label, version, size, media type, and provenance metadata;
   - bytes fetched separately only if the runner chooses.
6. Limits
   - maximum response/action bytes;
   - allowed action schemas;
   - optional runner-owned token or cost budget metadata.

This payload is structured JSON. SDKs may render it to a prompt, tool resource, or native agent state, but that rendering belongs to the runner.

## What makes a change relevant

Frozen releases use deterministic relevance, not semantic classification.

A frame is relevant when the Activity Pack explicitly emits it to that membership because:

- a field in its authorized projection changed;
- an Action Offer opened or closed;
- a personal deadline changed;
- another participant addressed it;
- evidence it owns or cited changed;
- a claim or plan it created changed status;
- a terminal result became visible.

An Attention Signal is even narrower: the pack declares that an enabled Agent Participant may need a fresh Invocation for a named reason.

WorldStream does not periodically scan all data with an LLM to decide whether an agent should run.

## Agent Heist example

Privacy-focused excerpt from a Navigator Invocation Context during Commitment. This is not the complete wire payload; the exact protocol context also carries the complete Head, schema/hash and authority/delivery witnesses, versioned runner budget/limits, and one delivery branch:

~~~json
{
  "activation": {
    "reason": "commitment_opened",
    "deadline": "2026-08-13T18:30:00Z"
  },
  "projection": {
    "core": {
      "room_status": "active",
      "membership": {
        "access_mode": "participant",
        "role": "navigator"
      }
    },
    "activity": {
      "phase": "commitment",
      "owned_clues": ["route-2"],
      "disclosed_clues": ["guard-1"],
      "public_plans": ["plan-7"],
      "private_offers": [],
      "own_commitment": null
    },
    "action_offers": [
      {
        "domain": "worldstream/action-offer/v1",
        "action_type": "commit_move",
        "payload_schema_digest": "blake3:...",
        "eligibility_window": {
          "opens_at": "2026-08-13T18:29:30Z",
          "deadline": "2026-08-13T18:30:00Z"
        }
      }
    ]
  },
  "changes_after_cursor": [
    {
      "reason": "commitment_opened",
      "cause_room_seq": 93
    }
  ],
  "artifact_references": []
}
~~~

It does not contain the Insider clue, Broker commitment, hidden facility state, complete raw log, or another model's reasoning.

## Investigation Room example

After a clock correction, the Timeline analyst receives the following privacy-focused Projection/artifact excerpt inside that same complete context envelope:

~~~json
{
  "activation": {
    "reason": "cited_evidence_superseded",
    "cause_room_seq": 147
  },
  "projection": {
    "core": {
      "room_status": "active",
      "membership": {
        "access_mode": "participant",
        "role": "timeline_analyst"
      }
    },
    "activity": {
      "phase": "review",
      "assigned_evidence": [
        {
          "digest": "blake3:new-correction",
          "version": "2",
          "supersedes": "blake3:old-timestamp"
        }
      ],
      "own_facts_marked_stale": ["fact-12", "fact-14"],
      "dependent_claims": ["claim-4"]
    },
    "action_offers": [
      {
        "domain": "worldstream/action-offer/v1",
        "action_type": "publish_fact",
        "payload_schema_digest": "blake3:...",
        "eligibility_window": null
      },
      {
        "domain": "worldstream/action-offer/v1",
        "action_type": "revise_claim",
        "payload_schema_digest": "blake3:...",
        "eligibility_window": null
      },
      {
        "domain": "worldstream/action-offer/v1",
        "action_type": "request_verification",
        "payload_schema_digest": "blake3:...",
        "eligibility_window": null
      }
    ]
  },
  "artifact_references": [
    {
      "digest": "blake3:new-correction",
      "label": "sensor-clock-correction",
      "media_type": "application/json",
      "size_bytes": 4812
    }
  ]
}
~~~

The server finds affected work by explicit evidence-to-fact-to-claim links in pack state. No vector search guesses the dependency.

## Evidence and provenance model

For v0.2, every published fact references exact evidence digests. Every claim references exact fact revisions. Superseding evidence never overwrites prior bytes.

Logical graph:

    Evidence version (immutable artifact digest plus pack metadata)
       → supports or contradicts Fact revision
           → supports or contradicts Claim revision
               → selected by Final Brief

Each edge has:

- source ID and revision;
- relationship type;
- author membership;
- creating room sequence;
- active, stale, withdrawn, or superseded state.

This graph enables deterministic invalidation and audit. It is activity state, not a general enterprise knowledge graph.

## Storage growth and retention

### Transitions

Retain canonical transitions for the room lifetime in the frozen releases. Use compact canonical payloads and prohibit large artifact bytes in events.

Host operators may archive/export Rooms whose Activities reached a Terminal Phase. Destructive Room deletion is an explicit local host-operator operation, not automatic garbage collection.

### Snapshots

Create one paired Core-and-Activity snapshot every 250 accepted Transitions or five active minutes in an idempotent postcommit job. Each pair binds one complete Room Head, both canonical values and component hashes, and the aggregate hash. Retain immutable Genesis independently and the latest three pairs. Every pair is replaceable; deleting all pairs and current materializations still permits Genesis-plus-Transition recovery.

### Observation frames

Retain acknowledged frames for a seven-day safety window, subject to a hard per-Membership ceiling of 10,000 frames or 64 MiB. A hard prune can include unacknowledged frames but forces an explicit Projection Reset; it never advances the Cursor or reuses a frame sequence.

### Artifacts

Deduplicate by digest. Enforce ten MiB per blob and one hundred MiB per Investigation room by default. An unreferenced temp upload is removed. A referenced immutable artifact remains while any retained room reference needs it.

### Derived indexes

Rebuild and prune freely. Never let deleting an index remove canonical evidence or change replay.

### Telemetry

Use external scraping/retention for metrics. WorldStream logs to stdout and does not turn telemetry into room memory.

## Authorization and retrieval

Authorization is evaluated before retrieval:

1. authenticate principal;
2. authorize room and membership;
3. determine viewer type and pack visibility;
4. filter artifact references and structured entities;
5. only then apply sorting, filtering, or optional text search;
6. log identifiers and counts, not private content.

Replay adds a historical gate after present authorization: reconstructed Membership existence, Standing, Access Mode, and Role at sequence N determine the viewer at N. A later Role or replacement Member ID inherits no earlier private content; spectator/operator and final-reveal history require explicit current policy.

Searching all evidence and filtering afterward is unsafe because result counts, snippets, timing, and errors can leak hidden data.

## Summary and compaction policy

No generated summaries are canonical in v0.1 or v0.2.

If an external agent publishes a summary:

- it is an ordinary typed claim or artifact;
- it names its sources;
- it can be challenged, superseded, or marked stale;
- the original evidence remains available under authorization;
- replay records that it was participant output, not server truth.

The server may generate deterministic structural summaries such as counts, Action Offers, deadlines, and changed IDs. It does not ask an LLM to compress the room.

## Required invariants

1. Canonical history, current materialization, operational integrity, Projection, Observation, Artifact, and Invocation Context are distinct.
2. Canonical transition history is never injected wholesale into a model by the server.
3. Authoritative current truth precedes historical similarity.
4. Authorization precedes persistence audience selection, retrieval, indexing, and rendering.
5. Every artifact reference names an immutable digest and exact visibility.
6. Superseding evidence creates a new version and explicit dependency invalidation.
7. Paired snapshots, current materializations, derived indexes, and caches cannot change Room truth or Replay.
8. A Projection Reset sends an authorized current Projection, not raw state; only matching `room.sync_ack` enters Live and never advances Cursor, while only separate `observation.ack` may advance Cursor.
9. WorldStream never stores chain-of-thought or provider credentials.
10. Agent-private memory remains runner-owned.
11. Context assembly is bounded and deterministic in the frozen releases.
12. Missing Runner availability does not delete Room data or an Activation Intent; Replay creates no Activation effect.
13. All three state hashes and the lineage hash reproduce from Genesis and Transitions after every cache is deleted.
14. Only a generation-fenced verifier restores healthy integrity, without rewriting canonical history.
15. Present-plus-historical authorization governs every Replay view.

## The simple explanation

WorldStream keeps the official shared scoreboard and history. When a human or AI needs to act, it receives only its current role-specific screen, the changes it missed, what it is allowed to do, and the exact evidence it may inspect.

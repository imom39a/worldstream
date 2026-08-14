# Context, State, and Memory

## Status

This document explains supporting data architecture. WorldStream is not positioned as a generic context layer, memory server, RAG system, or vector database.

The frozen rule is:

> WorldStream remembers the shared room exactly and gives each invocation a bounded authorized projection of that room.

Agent identity can be durable while every model invocation is ephemeral. The room persists; the model's hidden context does not.

## The core distinction

These concepts must not be collapsed:

### Core Room State

WorldStream-owned current facts about the Room lifecycle and Memberships, including Room Status, Access Modes, and current Role assignments. The Room Head is causal/history metadata, not part of this state value.

### Activity State

Activity Pack-owned current domain facts such as phase, clues, evidence versions, claims, deadlines, and Outcome. Legal Actions are derived from these facts, the current Membership, and pack rules.

### Authoritative Room State

The accepted current truth of the Room: Core Room State together with Activity State. Clients receive authorized Projections, never this aggregate directly.

### Canonical history

The immutable Genesis followed by ordered Transitions, including each accepted Stimulus and deterministic result, from which state can be reconstructed and verified.

### Projection

The current subset of Authoritative Room State that one Membership is authorized to see.

### Observation stream

The bounded Membership-specific changes after a Cursor.

### Artifact

An immutable external blob, introduced for Investigation Room, referenced by content digest and structured metadata.

### Invocation context

A temporary SDK payload assembled for one fresh agent run from current projection, relevant frames, legal actions, deadline, activation cause, and authorized artifact references.

### Agent-private memory

Anything a runner or agent chooses to keep outside WorldStream: preferences, summaries, embeddings, model sessions, private notes, or tool history. The server does not own it.

## Why canonical history or Markdown is not the prompt

Canonical transition history is good for truth and recovery but poor model input:

- it grows without bound;
- transitions and their domain events include superseded facts and operational detail;
- another participant may not be authorized to see them;
- many changes are irrelevant to the next action;
- a raw transcript obscures current legal actions and deadlines.

A Markdown file is useful as an Artifact or human-readable export, but it is not an Authoritative Room State format. Unstructured text makes deterministic invalidation, precise authorization, and Replay harder.

An embedding index can retrieve semantically similar text, but similarity is not authority, freshness, visibility, or causal dependency. It is therefore absent from v0.1 and v0.2.

## Frozen data planes

~~~mermaid
flowchart TD
    H["Canonical History: Genesis plus Transitions"] --> S["Current Authoritative Room State"]
    H --> SN["Snapshots for recovery"]
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
- exact Activity Pack revision and Activity State hashes;
- no token fragments, connection heartbeats, debug traces, or chain-of-thought;
- retained for the room lifetime in frozen releases.

The history answers what happened, not what an agent should read wholesale.

### 2. Current Authoritative Room State

Purpose:

- current core and activity truth;
- action validation;
- timer and dependency rules;
- source for authorized projections.

The active Room actor keeps canonical Activity State together with a current view of Core Room State. Immutable genesis plus Transitions reconstruct the aggregate; snapshots accelerate reloading Activity State while durable core records are verified against the same Room head.

Canonical Activity State is pack-defined and bounded to two MiB by default. Large evidence bytes do not belong in it; Activity State holds digests and metadata.

### 3. Authorized projections

Purpose:

- current authorized situation;
- current legal actions;
- deadlines and obligations;
- bounded model- and UI-ready structure.

A projection is regenerated from current state and viewer identity. It is not a permanently growing transcript.

The pack, not a generic LLM, decides what each role may see. Projection code is deterministic and covered by privacy tests.

### 4. Membership observation frames

Purpose:

- live incremental delivery;
- exact reconnect from a membership cursor;
- activation-relevant change range.

Frames are materialized by explicit audience. They may be pruned after acknowledgement plus a retention window because an authorized current projection can reset an old cursor.

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
- never included in an Activity State hash.

FTS5 may be evaluated after the structured Investigation fixture works. It is not a release requirement. Vector indexes and semantic summary stores are explicitly out of scope.

### 7. Ephemeral operational data

Connection presence, heartbeat timing, in-memory send queues, temporary upload progress, and metrics samples are operational. They do not consume canonical room sequence.

Runner availability is also operational. A durable activation remains pending even if no runner connection exists.

## Invocation-context assembly

WorldStream does not let an LLM invent a context query. It assembles a small contractually defined payload after an activation claim or room attachment.

Order of precedence:

1. Identity and cause
   - Room, Membership, Role, and Principal kind;
   - activation ID and typed reason;
   - cause room sequence and relevant frame range.
2. Current truth
   - pack-authorized current projection;
   - current phase and state version;
   - current legal actions.
3. Time constraints
   - personal and room deadlines;
   - activation lease expiry;
   - action based-on sequence.
4. Relevant changes
   - retained observation frames after the membership cursor;
   - or an explicit projection reset if that range was pruned.
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
- a legal action opened or closed;
- a personal deadline changed;
- another participant addressed it;
- evidence it owns or cited changed;
- a claim or plan it created changed status;
- a terminal result became visible.

An Attention Signal is even narrower: the pack declares that an enabled Agent Participant may need a fresh Invocation for a named reason.

WorldStream does not periodically scan all data with an LLM to decide whether an agent should run.

## Agent Heist example

Navigator invocation context during Commitment:

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
      "own_commitment": null,
      "legal_actions": ["commit_move"]
    }
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

After a clock correction, the Timeline analyst receives:

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
      "dependent_claims": ["claim-4"],
      "legal_actions": [
        "publish_fact",
        "revise_claim",
        "request_verification"
      ]
    }
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

Create snapshots every 250 accepted transitions or five active minutes. Retain immutable canonical genesis independently and the latest three automatic snapshots. Every snapshot is replaceable and can be recomputed.

### Observation frames

Retain unacknowledged frames. After acknowledgement, retain a seven-day safety window or a configured maximum per audience. Old cursors receive a projection reset.

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

Searching all evidence and filtering afterward is unsafe because result counts, snippets, timing, and errors can leak hidden data.

## Summary and compaction policy

No generated summaries are canonical in v0.1 or v0.2.

If an external agent publishes a summary:

- it is an ordinary typed claim or artifact;
- it names its sources;
- it can be challenged, superseded, or marked stale;
- the original evidence remains available under authorization;
- replay records that it was participant output, not server truth.

The server may generate deterministic structural summaries such as counts, legal actions, deadlines, and changed IDs. It does not ask an LLM to compress the room.

## Required invariants

1. Canonical history, current state, projection, observation, artifact, and invocation context are distinct.
2. Canonical transition history is never injected wholesale into a model by the server.
3. Authoritative current truth precedes historical similarity.
4. Authorization precedes persistence audience selection, retrieval, indexing, and rendering.
5. Every artifact reference names an immutable digest and exact visibility.
6. Superseding evidence creates a new version and explicit dependency invalidation.
7. Derived indexes and caches cannot change room truth or replay.
8. A cursor reset sends an authorized current projection, not raw state.
9. WorldStream never stores chain-of-thought or provider credentials.
10. Agent-private memory remains runner-owned.
11. Context assembly is bounded and deterministic in the frozen releases.
12. Missing runner availability does not delete room data or activation intent.

## The simple explanation

WorldStream keeps the official shared scoreboard and history. When a human or AI needs to act, it receives only its current role-specific screen, the changes it missed, what it is allowed to do, and the exact evidence it may inspect.

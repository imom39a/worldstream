# ADR 0005: Canonical Core State, Integrity, and Hash Lineage

Status: Accepted, 2026-08-15

WorldStream makes immutable Genesis plus accepted Transitions the sole canonical Room lineage. `CoreRoomState v1` is exactly Room Status plus the semantic Membership map; Activity State remains pack-owned; and their pair is Authoritative Room State. Durable Room Integrity State is generation-fenced operational state outside that pair, Room order, Replay state, and canonical hashes. This boundary makes both reducers replayable, lets every serving cache be discarded, and permits integrity repair without granting repair code authority to rewrite history.

## Decision

### Core ownership and reduction

The versioned WorldStream Core reducer exclusively owns:

- Room Status: `active | archived`;
- a canonically sorted Membership map keyed by immutable Member ID;
- for each Membership, immutable Principal ID and room-local Principal kind;
- Membership Standing: `enabled | suspended | departed`;
- Access Mode: `participant | spectator | operator`;
- exactly one current pack-defined Role for participant access and no Role for spectator/operator access.

Enabled and suspended are mutually reachable. Departed is terminal, and rejoining creates a new Member ID with no inherited Cursor or private frame stream. At most one non-departed Membership for a Principal may exist in a Room. Suspended and departed Memberships cannot attach, act, receive new frames, or be activated.

A normalized `CoreStimulusV1` records its kind, attributable authority, idempotency identity, exact expected Room sequence, reason code, semantic recorded time when applicable, and an unambiguous canonical Core before/after changeset. A Membership changeset is sorted by Member ID and contains at most one typed component per affected ID. Component kinds are Join, Resume, AccessModeChange, RoleChange, Suspend, or Depart. Archive is its own top-level kind and cannot occur inside MembershipChangeSet. The Core reducer validates the complete final state and pack Role cardinality as one value, so an atomic Role swap never persists, hashes, or exposes an invalid intermediate assignment.

Packs receive immutable Core-before and proposed-Core-after views. Join, Resume, AccessModeChange, and RoleChange components are vetoable; Suspend and Depart components are mandatory. The host rejects a MembershipChangeSet mixing those classes before pack entry, with no pack call, Transition, or receipt. A homogeneous set inherits its component class: all-vetoable may return one declared stable rejection for the entire atomic proposal, while all-mandatory must Apply and a Reject is PackFault. An allowed veto commits an idempotent administrative rejection but no Transition and is not a Pack Fault. A request whose normalized desired state was already true may instead record NoChange before pack application. Archive is independently mandatory: a declared pack veto is a Pack Fault, while immediate operational capability revocation remains available if canonical mutation is temporarily fenced. Packs never mutate or duplicate Core ownership.

Archive is irreversible. It is an ordered Core Transition, never an automatic consequence of a terminal Activity Phase. In the winning Room order it cancels scheduled timers and generation-fences pending and leased Activation work atomically. A healthy archived Room permits authorized reads, export, Replay, and ordered suspend/depart only; it permits no join, resume, participant work, or Access/Role elevation.

### Canonical hashes and complete Head

The hash functions are domain-separated BLAKE3 over canonical typed values. Concrete field names, framing, and domain-tag strings belong to the versioned codec contract, but the following semantic inclusions are exact:

- `core_state_hash = CoreHash(core_schema_version, canonical_core_state)`;
- `activity_state_hash = ActivityHash(pack_digest, canonical_activity_state)`;
- `authoritative_state_hash = AuthoritativeHash(core_schema_version, pack_digest, core_state_hash, activity_state_hash)`.

The aggregate hash binds both component hashes; it does not replace either one.

Genesis is sequence zero. Its immutable record stores the exact reconstructible creation inputs, initial Core and Activity values, normalized initial timer requests, and three initial state hashes. `genesis_hash` binds:

- Room identity and Genesis/hash/codec version identities;
- Core schema version and exact pack digest;
- canonical Room configuration;
- Room seed and logical creation time;
- normalized ordered initial timer requests;
- initial Core, Activity, and aggregate Authoritative State hashes.

Every accepted Transition after Genesis has one `transition_hash` binding exactly:

- Room identity, Room sequence, and Transition/hash/codec version identities;
- Core schema version and exact pack digest;
- the previous Transition hash, or the Genesis hash at sequence one;
- the complete normalized recorded Stimulus;
- ordered Domain Events;
- normalized ordered timer changes;
- deterministic ordered Attention Signals;
- resulting Core, Activity, and aggregate Authoritative State hashes.

Canonical authority attribution, idempotency identity, expected sequence, reason, and semantic-time/input fields inside the normalized Stimulus are therefore included. The following are excluded: Room Integrity State, integrity generation and incident/repair audit; bearer secrets and operational authorization/commit witnesses; receipts and disposition rows; current materializations; snapshot bytes, encoding, cadence, and creation time; Projections, frames, frame heads/floors, Cursors, resets, Sessions, and delivery; Activation policy versions/decisions, intents, offers, leases, attempts, completion, and Invocation Context; diagnostics, telemetry, and commit wall time.

The complete Room Head at sequence N is:

`(room_id, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash)`.

Every accepted Transition installs all Head fields together. Genesis, Transitions, paired snapshots, and current materializations must agree with the applicable complete Head.

### Materialization and snapshots

Current Room, Membership, and Activity rows and actor memory are verified materializations of canonical lineage, never a second authority. A snapshot is one paired Core-and-Activity postcommit cache at one Room sequence. It binds the applicable lineage hash, Core schema version, pack digest, both canonical state values and component hashes, and aggregate hash.

Snapshot creation is idempotent and happens only after the canonical commit; its failure cannot roll back or change that commit. A corrupt pair is discarded as a whole. Recovery may use a verified pair plus the Transition tail, but deleting every snapshot and current materialization must still allow reconstruction from Genesis and all Transitions.

### Integrity, serving, and repair

Room Integrity State is durable operational state with a monotonically increasing integrity generation and a separate append-only incident/repair audit:

- `healthy`: canonical lineage and required executors verify, so the Room may advance;
- `faulted`: the last canonical Head verifies, but the runtime cannot safely advance it;
- `quarantined`: canonical integrity cannot be established.

Every new Existing Advance or durable disposition atomically rechecks both `healthy` and the unchanged integrity generation. A lost race commits no Transition, Room sequence, or receipt. Create has no pre-existing Room Integrity witness: its all-or-none bundle initializes operational state exactly `healthy` at generation `1`. Faulted and quarantined Rooms accept no canonical participant or administrative mutation. Capability revocation, diagnostics, raw export, restore, and verifier repair remain operational.

A faulted Room may serve only its last verified authorized Projection, retained Frame Catch-up, and verified Replay, always with an explicit integrity envelope. A quarantined Room serves no normal Projection, Catch-up, or claimed-current Replay; only authenticated host-operator diagnostics, raw export, restore, and verification remain available.

An operator may request repair, but cannot mark a Room healthy. Only a successful verifier operation matching the current integrity generation may do so. Repair may rebuild snapshots/materializations, reinstall the exact retained pack, or restore exact canonical bytes from a verified backup. It never edits, skips, reorders, synthesizes, or silently replaces Genesis or a Transition.

### Replay authorization

Replay first authenticates and authorizes the present requester. The reconstructed Core Membership, Standing, Access Mode, and Role at requested sequence N then select the historical viewer passed to the exact pack. A Membership absent at N receives no participant/private view at N. A later Role assignment or replacement Membership never inherits a former Membership's historical private data. Spectator/operator history and final reveal require explicit presently authorized projection policy and never bypass pack privacy.

Replay runs the same versioned Core reducer and exact retained pack reducer used for live lineage, verifies all three state hashes and the Genesis/Transition chain, and performs no delivery, policy evaluation, Activation, timer scheduling, or external effect.

## Worked Genesis → Core change → Replay example

The symbolic digest names below stand for full BLAKE3 values so the example does not prematurely freeze codec framing.

Room `room-7` pins Core schema `core/v1` and pack digest `P`. Genesis contains an active Room and this sorted Membership map:

~~~json
{
  "m-insider": {
    "member_id": "m-insider",
    "principal_id": "p-2",
    "principal_kind": "agent",
    "standing": "enabled",
    "access_mode": "participant",
    "role": "insider"
  },
  "m-navigator": {
    "member_id": "m-navigator",
    "principal_id": "p-1",
    "principal_kind": "agent",
    "standing": "enabled",
    "access_mode": "participant",
    "role": "navigator"
  }
}
~~~

The pack initializes Activity State `A0 = {"phase":"briefing","published_clues":[]}` and no timers. The host computes `C0 = CoreHash(core/v1, Core0)`, `A0h = ActivityHash(P, A0)`, and `S0 = AuthoritativeHash(core/v1, P, C0, A0h)`. Genesis binds those three hashes and the empty normalized timer list, producing `G`; Head zero becomes `(room-7, 0, G, core/v1, P, C0, A0h, S0)`. No snapshot or Observation Frame is needed to establish Genesis.

At expected sequence zero an authorized administrator submits one Core Stimulus with idempotency key `swap-1`, reason `operator.role_swap`, and these Member-ID-sorted pairs:

~~~json
[
  {
    "member_id": "m-insider",
    "before": {"standing":"enabled","access_mode":"participant","role":"insider"},
    "after":  {"standing":"enabled","access_mode":"participant","role":"navigator"}
  },
  {
    "member_id": "m-navigator",
    "before": {"standing":"enabled","access_mode":"participant","role":"navigator"},
    "after":  {"standing":"enabled","access_mode":"participant","role":"insider"}
  }
]
~~~

The Core reducer validates the final cardinality once; it never exposes a state with two Navigators or two Insiders. The pack accepts the immutable Core proposal, leaves Activity bytes unchanged, emits one ordered `roles_swapped` Domain Event, and emits no timer change or Attention Signal. The host computes `C1`, reuses `A0h`, computes aggregate `S1`, and calculates `T1` over sequence one, `G`, the complete normalized Core Stimulus, the one event, both empty ordered output lists, and `(C1, A0h, S1)`. Head one becomes `(room-7, 1, T1, core/v1, P, C1, A0h, S1)`.

A presently authorized Replay of sequence zero folds Genesis and passes `m-navigator` to the pack as the historical Navigator viewer. Replay of sequence one folds the same Core Stimulus and passes that Membership as the historical Insider viewer. Its current Insider Role does not reveal Insider-only data at sequence zero. A replacement Membership created after sequence one is absent at both requested sequences and receives no participant/private view. Replay reproduces `C0/A0h/S0/G` or `C1/A0h/S1/T1` without creating frames, reevaluating Activation policy, or contacting a Runner.

## Considered options

- Hashing and snapshotting Activity State alone was rejected because Core changes would be only loosely checked and could not be replayed as equal canonical state.
- Putting integrity inside Core was rejected because detecting or repairing a fault would itself require a canonical Transition while the Room is unsafe to advance.
- Treating current rows or snapshots as authoritative was rejected because corruption or cache loss would create competing truth and prevent Genesis-only recovery.
- Applying multi-Membership changes sequentially was rejected because valid final swaps can require invalid intermediate Role cardinality.
- Repair by editing or skipping history was rejected because it destroys auditability, idempotent results, and deterministic Replay.

## Consequences

Implementations must retain exact Core and pack reducers/codecs for every live lineage, maintain three hashes plus a lineage hash, verify paired checkpoints and serving materializations, and fence commits on integrity generation. This adds storage and conformance work, but yields one reconstructible truth, component-level fault isolation, privacy-correct historical Replay, disposable caches, and a repair path that cannot normalize history after the fact.

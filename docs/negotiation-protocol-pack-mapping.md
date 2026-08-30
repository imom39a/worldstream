# Mapping A202 and A2CN onto `ActivityPackV1`

> - **Status:** Non-normative decision research for “Map A202 and A2CN onto ActivityPackV1”
> - **Current as of:** 2026-08-30
> - **WorldStream baseline:** [`046c876`](https://github.com/imom39a/worldstream/tree/046c87648d02953ba8e23fa35bc6b8764a2e6833)
> - **Decision authority:** This document changes no ADR, requirement, protocol, or implementation.

## Decision

One deliberately narrow A202 integration fits the existing Room Kernel without changing its semantics:

> **WorldStream A202 v0.1 Operated Single-Session Formation Compatibility Profile**

This is a human-readable WorldStream compatibility target, not a new protocol identifier and not an A202-issued conformance scope. It pins A202’s existing `a202-commercial/0.1` object model, state-machine rules version 1.3, the official `a202-profile/calibration-service/0.1` transaction profile for the conformance demonstration, and the operator capabilities relevant to one transaction with exactly one bilateral negotiation session.

The fit is **conditional but real**:

- `ActivityPackV1` can own the deterministic negotiation state, guards, logical A202 stream heads, Roles, Action Offers, timers, projections, and evidence index.
- The existing Room Kernel can own admission, one authoritative Room order, atomic commit, idempotency, persistence, reconnect, activation, and Replay.
- Protocol adapters and signers must remain outside the pack. They preserve A202 canonical bytes, resolve keys and mandates, prepare events under the exact appender identity A202 assigns to that act, and submit ordinary authenticated WorldStream Actions. They never become a second state authority.
- The pack must verify every signed object and event before applying it. It must not hold a private signing key or call a network service.
- One Room may contain one A202 transaction and one A202 session. A competitive transaction with several confidential supplier sessions does **not** fit in one Room, because WorldStream has one Room Head while A202 requires independently sequenced streams that do not leak rival activity.

The word “compatibility” is essential. A202 formally separates `a202-scope/bilateral/0.1` from `a202-scope/operated/0.1`, and one conformance grade may name only one scope. Offer signing, mandate verification, exact approvals, acceptance, agreement formation, and evidence verification are largely in the bilateral scope; session-stream ordering and control-plane annotations are in the operated scope. A complete WorldStream deal therefore crosses both scopes and would eventually need separate evidence for each. Calling the first implementation “A202 operated conformant” before that evidence exists would overclaim. A202’s scope rules expressly say an operated grade claims nothing about unassessed bilateral capabilities and that a grade covers exactly one role scope ([scope partition](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/conformance-role-scopes-v0.1.md#L42-L63), [one-grade/one-scope rule](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/conformance-role-scopes-v0.1.md#L215-L225)).

A2CN is the **better mechanical comparison**, not the better first protocol target. Its single bilateral session sequence, explicit turn-taking, offer/counter/accept/withdraw messages, approval receipt, deterministic transaction record, and audit log map closely to one Room. But A2CN says each party’s local state is authoritative, resolves divergence between two party-held states, and explicitly excludes real-time streaming negotiations. Making WorldStream the sole authority would change that trust and failure model rather than merely operate a missing layer ([A2CN scope and terminology](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L170-L203), [A2CN session authority and divergence](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L1330-L1403)). Use A2CN to benchmark developer ergonomics, message validation, human approval, and export. Do not switch to it merely because its first demo is easier.

## Source and reproduction record

Only primary sources were used.

| Source | Pinned revision | Material inspected |
|---|---|---|
| WorldStream | [`046c876`](https://github.com/imom39a/worldstream/tree/046c87648d02953ba8e23fa35bc6b8764a2e6833) | Root domain model, accepted ADRs, protocol/activity-pack docs, and Rust implementation |
| A202 | [`fa85aa8`](https://github.com/a202-protocol/a202/tree/fa85aa8b49bfe7b3f7ded487c98500a600e92e41) | Canonical model, state machines, mandate, role scopes, schemas, bindings, evidence procedure, reference implementation, fixtures |
| A2CN | [`ff6305e`](https://github.com/A2CN-protocol/A2CN/tree/ff6305eda5b1bd545a0c83596e6419bfc2985b07) | v0.2 specification, normative schemas, Python reference responder, state manager, storage adapters |

Both upstream repository HEADs were independently re-resolved on 2026-08-30 and matched the pinned commits above. The official A202 conformance runner was executed at `fa85aa8` in a clean temporary environment with `jsonschema 4.26.0`: **148 passed, 0 failed** (32 positive and 116 negative). That reproduces the upstream fixture set; it is not evidence that WorldStream passes it. The runner and manifest are the authoritative upstream artifacts ([runner](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/run-conformance.py), [manifest](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/manifest-v0.1.json)).

A202 labels the mapped state machine an **experimental working specification**. The pin makes this research reproducible; it does not make the upstream protocol stable ([A202 state-machine status](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L1-L11)).

The earlier working-tree product note, `docs/negotiation-product-research.md`, was used as prior context only. It is not a source for this artifact; every protocol conclusion below was rechecked against the pinned sources.

## What `ActivityPackV1` already provides

The current pack seam has exactly five operations: `descriptor`, `initialize`, `reduce`, `view`, and `observe`. A pack receives canonical, deterministic input and has no clock, network, storage, filesystem, scheduler, session, delivery, or activation capability ([ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md), [`ActivityPackV1`](../crates/worldstream-core/src/activity_pack.rs#L57-L110)). That is the right authority boundary for negotiation rules.

| Need | Existing owner | Mapping |
|---|---|---|
| Immutable protocol/rule identity | Pack registry and Room-pinned pack digest | Pin the exact A202 spec snapshot, rules version, supported object families, schemas, transaction profile, and crypto dependencies in the pack revision lock. |
| Initial parties and session | `initialize` plus immutable Core Memberships | Validate one buyer agent, one seller agent, one human approver, and one venue-signing agent; initialize one transaction and one session. |
| Legal next moves | `view` and canonical Action Offers | Offer only the Actions legal for the viewer’s Role, current A202 states, exact current proposal, and deadline. |
| State transition | Pure `reduce` over one recorded Stimulus | Validate the A202 objects, signatures, authority, hashes, stream sequence, state guard, and acting Membership; emit the complete next Activity State. |
| Deadlines | Host-owned timer generations and recorded scheduled time | Enforce Offer validity and state-sensitive formation expiry without an ambient clock. |
| Human and agent attention | Membership observations for humans; deterministic Attention Signals and durable Activation Intents for agents | Update the human approver’s addressed Observation Stream/UI; activate the venue-signing agent. External processes remain ephemeral. |
| Private/current views | Membership-scoped `view` | Give each Membership only its authorized projection and exact current Action Offers. |
| Live deltas | Membership-scoped `observe` | Emit at most one coalesced authorized observation for the accepted Transition. |
| Durable ordering | Room sequence and Complete Head | Serialize all accepted Room changes and reject stale Action bases. |
| Atomicity and retries | Backend-neutral Room Commit | Commit state, lineage, timers, observations, activation decisions, and receipt together; resolve unknown commits by operation identity. |
| Disconnect and return | Observation Stream, Cursor, Catch-up, Projection Reset | Resume human and agent participants without reconstructing state from chat history. |
| Historical verification | Exact retained reducer and Replay | Recompute WorldStream state and hashes with the original pack revision. |

WorldStream already makes the database commit the sole linearization point and atomically stores the Transition, Complete Head, Activity State, timers, addressed frames, activation work, and receipt ([ADR 0006](adr/0006-backend-neutral-atomic-room-commit.md)). Each Membership already has its own durable Observation Stream, Cursor, catch-up range, and reset barrier ([ADR 0008](adr/0008-membership-observation-streams-and-reset-barriers.md)). Those are operator properties that neither protocol should be asked to reimplement.

The host contract validates that an Attention target is an enabled Agent Participant, so the Human approver is notified only through its addressed observation/UI path; a pack that targets that Human with Attention would fault ([Attention target validation](../crates/worldstream-core/src/activity_pack.rs#L3028-L3051)).

### Terminology collision

WorldStream’s **Action Offer** is the canonical statement that a participant may currently submit one typed Action. It is not a commercial offer ([`ActionOfferV1`](../crates/worldstream-core/src/activity_pack.rs#L453-L489)). In this pack:

- **Proposal Revision** is the product/domain term for one transmitted set of commercial terms.
- An A202 Proposal Revision is an immutable A202 `Offer`.
- A counter is another A202 `Offer` whose `supersedes_offer_id` names the immediately preceding Offer.
- `submit_proposal_revision` is a WorldStream Action type.
- The Action Offer named `submit_proposal_revision` says that Action is currently legal; it contains no commercial terms.

This vocabulary should be used in UI and documentation to avoid saying “the Offer offers an offer.”

## The selected A202 compatibility profile

### Exact name and pins

**Name:** WorldStream A202 v0.1 Operated Single-Session Formation Compatibility Profile

**Pinned protocol material:**

- A202 repository revision `fa85aa8b49bfe7b3f7ded487c98500a600e92e41`.
- Shared-object `spec_version`: `a202-commercial/0.1`.
- State-machine rules version: `1.3`.
- Operated role-scope reference: `a202-scope/operated/0.1`.
- Required bilateral object behavior: `a202-scope/bilateral/0.1` for mandates, Offers, Acceptance, Approval, Agreement, signatures, and evidence.
- Demonstration transaction profile: the upstream `a202-profile/calibration-service/0.1`, not a WorldStream-invented terms protocol.
- Exactly one A202 transaction, exactly one A202 session, exactly two commercial organizations, and exactly one WorldStream Room.

A202’s canonical model is market-neutral: the shared terms envelope contains a profile identifier, fixed core terms, and profile-owned terms; the kernel must not gain profile-specific fields ([canonical model §8](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#L269-L282)). Using the upstream calibration profile proves the mapping without inventing a new transaction profile. A future AI-capacity profile would need its own A202 registration/versioning work and is not part of this ticket.

### Included lifecycle

The compatibility profile includes:

1. An already-qualified, already-opened, two-party operated session imported at Room Genesis with its exact signed A202 opening history.
2. Signed and authorized Offers.
3. Immutable counteroffers through `supersedes_offer_id`.
4. Withdrawal before acceptance.
5. Offer validity plus one pinned formation deadline used as both the A202 session deadline and transaction deadline.
6. One exact human approval of the buyer’s acceptance act.
7. Acceptance over the exact current Offer hash.
8. Operated selection of the accepted Offer.
9. A dual-signed Agreement over the accepted terms.
10. `agreement.committed`, producing a successful WorldStream Outcome while retaining the pinned A202 session at `accepted` pending an upstream closure repair.
11. Per-stream A202 evidence plus WorldStream lineage evidence.

A202 expressly separates acceptance, selection, and agreement. Acceptance moves the session to `accepted`; operated selection then moves the transaction to `agreement_pending`; commitment requires approvals complete and both parties to sign the same Agreement hash ([session/aggregate relationship](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L188-L198), [agreement formation](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#L323-L347)). The pack must preserve those distinctions rather than turning `accept` into an immediate contract.

The selected procurement path is intentionally asymmetric: both parties may issue a superseding Proposal Revision, but the final current Offer must be seller-authored and the buyer is its accepting party. That makes the buyer’s exact human approval mandatory and testable. If the current Offer is buyer-authored, the seller may counter but this profile does not offer seller acceptance; only the buyer, as that Offer’s author, may withdraw it. That is a declared restriction to an existing A202 path, not a new transition; a future symmetric profile would need an equally explicit approval policy for seller acceptance.

### Explicitly excluded

- Directory publication and discovery.
- Invitation onboarding.
- Qualification.
- More than one counterparty session.
- Auctions and competitive awards.
- Cross-Room transaction aggregation.
- Obligations, performance, settlement, disputes, determinations, amendments, and termination after commitment.
- Operator custody of either commercial party’s signing key.
- A claim of either A202 role-scope conformance.
- A new WorldStream negotiation wire protocol.
- A successful `session.closed` event under the pinned A202 schema; the aggregate formation still reaches `committed`.

These exclusions matter because the full official operated scope includes invitation onboarding, operator key custody, concurrent-counterparty isolation, awards, publication/qualification, operated determinations, and control-plane annotations ([A202 operated capabilities](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/conformance-role-scopes-v0.1.md#L155-L211)). A single-session subset cannot truthfully stand in for that whole assessment surface.

## One Room and two logical A202 streams

A202 has a transaction aggregate stream and one session stream per counterparty. It requires optimistic concurrency per stream and warns that one counter shared across several confidential sessions leaks rival activity ([state-machine §§2, 8](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L13-L22), [per-stream rules](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L198-L257)).

The profile therefore uses three orders with different meanings:

| Order | Owner | Meaning |
|---|---|---|
| WorldStream `room_seq` | Room Kernel | Total order of every accepted canonical change in this Room. |
| A202 transaction sequence | Pack Activity State | Contiguous order of A202 transaction-stream events only. |
| A202 session sequence | Pack Activity State | Contiguous order of A202 events for this one bilateral session only. |

The pack stores independent A202 sequence and predecessor-hash heads and increments only the stream targeted by a valid A202 event. The WorldStream transition binds the exact signed event bytes and resulting logical heads. Genesis may not simply assert `negotiating`/`opened`: it must verify and retain a contiguous signed aggregate history through `request.published`, `qualification.started`, and `negotiation.opened`, plus the session-creation evidence and resulting heads. This imports an existing operated session into the Room; it does not invent a shortcut transition.

An ingress adapter supplies both:

- the A202 `expected_sequence` inside the signed Action Envelope for the targeted A202 stream; and
- the current WorldStream Complete Head as transport/admission basis, outside the A202 signed bytes.

If an unrelated logical stream advanced after preparation, WorldStream may reject the stale Room basis even though the A202 target stream did not change. The adapter may reprepare only the WorldStream submission against the new Head while retaining identical A202 bytes when that A202 stream head is unchanged. If the A202 stream changed, the party must construct and sign a new A202 act with the new `expected_sequence`. The adapter must never silently rewrite a signed A202 object.

This is a serialization refinement for one session, not a general solution for confidential concurrent sessions. A future multi-supplier transaction needs separate Rooms for supplier sessions plus an explicitly designed aggregate authority; putting them in one Room would expose Head movement and stale-basis conflicts. That is an architecture decision beyond this profile.

## Roles and authority

### WorldStream Roles

| Pack Role | Principal kind | Cardinality | May do |
|---|---:|---:|---|
| `buyer_agent` | Agent | exactly 1 | Submit and withdraw its own Proposal Revisions; request exact approval; accept/select the current seller Offer; submit the dual-signed Agreement and buyer-authored commitment event. |
| `seller_agent` | Agent | exactly 1 | Submit and withdraw its own Proposal Revisions; submit seller-side Agreement signature material. |
| `buyer_approver` | Human | exactly 1 | Approve or reject one exact pending buyer act; never propose terms or sign as the buyer agent. |
| `venue_signer` | Agent | exactly 1 | Submit control-plane-signed A202 events for timer-caused expiry and other venue-only transitions; never propose or accept commercial terms. |

Spectator and operator access remain WorldStream Access Modes, not pack Roles. A WorldStream Operator Membership is read-only and is not the A202 control plane. The `venue_signer` is a narrowly authorized participant because an external process must sometimes submit a signed protocol object as an Action; its Role grants only those pack Actions.

The descriptor can enforce Role cardinality. The pack must additionally enforce Principal kind and A202 identity bindings at Genesis and veto later membership/Role changes that would break them. WorldStream Core deliberately keeps Principal kind, Access Mode, and pack Role separate ([root domain model](../CONTEXT.md)).

### Two independent authority checks

WorldStream authority and A202 commercial authority are complementary, not interchangeable:

1. **WorldStream admission authority** proves that this authenticated Principal may act through this enabled Membership in this Room.
2. **A202 commercial authority** proves that the embedded object was signed by the declared actor, under a valid key and mandate, for this transaction, action, counterparty, amount, time, and purpose.

The pack must require both. A stolen WorldStream participant bearer must not be enough to forge a commercial Offer, and a valid A202 signature held by the wrong Room Membership must not be enough to act through someone else’s seat.

A202 mandates bound actions, scope, constraints, delegation, approval rules, and validity; exact approval binds an action hash, transaction, approver identity and Role, decision, creation/expiry, conditions, and approver signature ([mandate evaluation and approval](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/authority/commercial-mandate-v0.1.md#L232-L303)). A202 also requires key status to be resolved at signing time and verification time ([canonical signatures](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#L135-L145)).

The pack has no network capability, so DID/key/mandate/status resolution occurs in a configured resolver outside it. Genesis pins resolver trust anchors, origin-validation rules, and one predefined resolver-status source. Before a commercial act, that resolver authenticates the upstream origin and submits the exact response bytes, resolved-document digest, subject, status, retrieval time, validity metadata, and chain as an authenticated `ExternalInputV1`. The host authorizes that exact source/input before pack entry; the pack records it and rechecks all deterministic document signatures and bindings. For every object signature, retained resolution evidence must cover its protected `signed_at`; the latest resolution must also remain fresh at the subsequent participant Action’s `admitted_at`, and that Action must use the exact Room Head containing the update. Where the upstream response is itself signed, those exact signature bytes are retained too. Content addressing alone proves only byte integrity, not source authenticity or freshness, and is insufficient. The pack independently checks canonical hashes, object signatures, key reference, validity interval, transaction/party binding, action scope, amount limits, approval binding, and the current authenticated status input. A live lookup result that is neither authenticated nor recorded cannot authorize reduction.

For each state-changing A202 event, the profile pins the exact appender organization/agent, mandate, key, signature purpose, Action Envelope, and signed `allow` Policy Decision. It does not flatten every event into a venue signature:

- the operated session events `offer.submitted`, `offer.accepted`, and `offer.withdrawn` are appended by the configured control-plane agent;
- `offer.selected` and `agreement.committed` are appended by the buyer agent under its selection/commitment authority; and
- deadline and aggregate-driven session-close events are appended by the configured venue-signing agent acting as the authoritative clock/control plane.

This split follows the upstream operated session fixtures and buyer-authored selection fixture, plus the state-machine rule that the buyer appends `offer.selected`; the official direct-formation evidence likewise shows a buyer-authored `agreement.committed` event ([operated session evidence](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/fixtures/v0.1/valid-session-verification-report-partial-disclosure.json#L40-L121), [buyer selection event](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/fixtures/v0.1/valid-transaction-event-references.json#L1-L42), [operated selection rule](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L188-L198), [buyer commitment event](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/conformance/fixtures/v0.1/valid-agreement-direct-formation.json#L232-L277)).

For an ordinary commercial Action, the outside adapter constructs one composite submission: the party-signed Action Envelope and commercial object, the acting party’s signed Policy Decision, and the correctly authored event candidate over the current logical A202 head. The authenticated buyer or seller Membership submits that composite as one WorldStream Action. The extra signature is embedded evidence, not a second Room mutation. The pack either verifies and applies the whole composite atomically or rejects it. If the logical A202 head changes before admission, the event must be prepared and signed again; the adapter may never rewrite signed bytes. The `venue_signer` Membership is needed when no commercial participant Action naturally carries the venue event, notably deadline resolution.

## Preserving signed bytes

A202 objects use RFC 8785 JSON Canonicalization Scheme and SHA-256 content hashes over canonical object bytes with `content_hash`, `signatures`, and `kernel_annotations` omitted. A202’s signature input is more specific: those canonical object bytes, followed by one literal `.` byte, followed by the RFC 8785 bytes of that signature entry’s protected `{algorithm,key_id,purpose,signed_at}` object. Control-plane annotations are attached only after signing and remain outside both inputs ([canonical model §§3–4](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#L67-L145)). The A202 carrier binding is stricter still: the transmitted object is opaque, byte-identical canonical JSON; parsing and reserializing before verification verifies the receiver’s serializer rather than the sender’s bytes ([A2A binding §5](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/bindings/a2a-binding-v0.1.md#L123-L149)).

WorldStream canonical JSON is a different, narrower contract: it rejects floats and unsafe integers and has its own unique writer ([`CanonicalJsonV1`](../crates/worldstream-core/src/canonical.rs#L13-L94)). The official A202 profiles happen to use integer counters and decimal strings, but semantic overlap is not proof that every future A202 canonical byte sequence will round-trip identically.

The safe mapping is therefore byte preserving:

```json
{
  "media_type": "application/a202-commercial+json",
  "canonical_bytes_base64url": "...",
  "declared_content_hash": "..."
}
```

The pack Action schema validates bounded wrapper shape and length. The pack then:

1. base64url-decodes the bytes;
2. requires byte-for-byte A202 canonical form;
3. parses with the exact pinned A202 schema/profile set;
4. recomputes SHA-256 and the complete protected signature inputs, including the `.` plus protected-entry suffix;
5. verifies signatures and purposes;
6. verifies all cross-object and state-machine references;
7. stores the exact bytes, still byte-addressable, in bounded Activity State; and
8. indexes object ID and content hash separately for deterministic lookup.

For an Offer, the party submits canonical bytes without `kernel_annotations`. After the pack accepts the matching control-plane-signed event, it deterministically attaches `policy_decision_id`, A202 session ID, logical session sequence, and WorldStream Action `admitted_at` as A202 `received_at`, then stores the resulting canonical annotated-object bytes. Annotations do not invalidate the party signature because A202 excludes them from its signed/hash input.

The A202 canonicalizer, schema bundle, registered profile, signature verifier, and crypto dependencies become part of the immutable Activity Pack revision. A change to any of them produces a new pack digest. This follows the existing revision-retention rule rather than creating an updatable protocol engine inside a Room ([ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md)).

## Lifecycle mapping

### Activity State outline

The pack-owned state needs only bounded, typed facts:

```text
protocol_pin
  a202_commit
  spec_version
  rules_version
  transaction_profile

parties
  buyer Membership ↔ A202 organization/agent/mandate/key refs
  seller Membership ↔ A202 organization/agent/mandate/key refs
  approver Membership ↔ A202 principal/key/role refs
  venue Membership ↔ A202 control-plane/key/mandate refs

transaction
  id
  aggregate_state
  transaction_stream { next_sequence, last_event_hash, event_ids }

session
  id
  session_state
  session_stream { next_sequence, last_event_hash, event_ids }
  current_offer_id
  current_offer_hash
  current_offer_valid_until
  offer_acceptance_expired
  pending_exact_approval
  formation_deadline
  deadline_resolution_phase

objects
  object_id → { content_hash, canonical_bytes_base64url, visibility_class }

agreement
  accepted_offer_id/hash
  acceptance_id/hash
  agreement_id/hash
  committed_at_room_seq
```

Reservation values, scoring weights, negotiation tactics, prompts, chain-of-thought, draft offers, and untransmitted alternatives never enter this state. A202 explicitly keeps those data classes outside its shared kernel ([private strategy boundary](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#L419-L433)). They remain Agent-Private Memory.

### Transition table

| WorldStream Action or Stimulus | Actor | A202 material | Required checks | Result |
|---|---|---|---|---|
| Genesis | Host, then pack `initialize` | Exact signed operated opening history through `request.published`, `qualification.started`, and `negotiation.opened`, including session-creation evidence | Exact protocol pins; contiguous transaction stream; Role/kind/cardinality; two parties; one session; profile; key/mandate refs; one timestamp declared as both transaction and session formation deadline | Activity State imports the verified A202 aggregate `negotiating`, session `opened`, and exact logical heads; formation-deadline timer scheduled. |
| `submit_proposal_revision` | Either buyer or seller agent | Signed Action Envelope, signed Policy Decision `allow`, signed immutable Offer, prepared control-plane-signed `offer.submitted` event | Both authorities; actor/offeror binding; target session sequence; predecessor event hash; Offer signature/hash; complete profile terms; `supersedes_offer_id`; `valid_until` no later than formation deadline; exact Room Head | Append annotated Offer and session event atomically; set session `active`; update current Offer; schedule/reschedule validity timer; Attention to offeree. |
| `submit_proposal_revision` as counter | Offeree of current Offer | Same object families, new Offer | All above plus `supersedes_offer_id` equals immediately preceding Offer; new Offer is immutable | Append a Proposal Revision; prior Offer remains evidence. |
| `request_exact_approval` | Buyer agent | Canonical candidate Acceptance Action Envelope, referenced current Offer hash, and signed Policy Decision `require_approval`; no A202 stream event yet | Candidate is byte exact, targets current A202 session sequence, references current unexpired Offer, matches buyer Role, and the decision binds its `action_hash` | Pack state marks one pending approval; the human’s addressed Observation Stream exposes the approval card for UI notification; no Attention Signal or A202 stream sequence is consumed. |
| `record_exact_approval` | Human buyer approver | Signed A202 Approval object | WorldStream Membership is the configured human; A202 principal/key/role match; Approval transaction and `action_hash` equal pending candidate; decision; expiry; signature | Store exact Approval; mark approved or rejected; no A202 stream sequence consumed. |
| `accept_current_proposal` | Buyer agent | The byte-identical approved candidate Action Envelope, signed Approval, a fresh signed Policy Decision `allow`, signed Acceptance, and control-plane-signed `offer.accepted` event | Evaluator reran the unchanged envelope with Approval and current mandate/status; fresh `allow.action_hash` matches it; current Offer remains exact/unexpired; Acceptance signs Offer hash; session sequence/head current | Append Acceptance and session event; session becomes `accepted`; cancel Offer-validity timer. |
| `select_accepted_proposal` | Buyer agent | Buyer-signed selection Action Envelope, signed Policy Decision `allow`, and buyer-signed `offer.selected` transaction event | Every act/event reference binds; accepted Offer is current; buyer selection authority; transaction stream sequence/head | Aggregate becomes `agreement_pending`. No competitive-award claim is made. |
| `commit_agreement` | Buyer agent | Buyer-signed commitment Action Envelope, signed Policy Decision `allow`, dual-signed Agreement, and buyer-signed `agreement.committed` transaction event | Every act/event reference binds; Agreement names Acceptance; accepted Offer and terms hashes recompute; required exact Approval present; both signatures are valid over the same Agreement bytes | Aggregate becomes `committed`; cancel Offer and formation timers; expose a successful Outcome with agreement ID/hash; pinned A202 session remains `accepted`. |
| `withdraw_live_proposal` | Current Offer’s offeror | Signed Action Envelope, signed Policy Decision `allow`, and control-plane-signed `offer.withdrawn` event whose data is exactly `{"close_reason":"withdrawn_by_offeror"}` | Before Acceptance; actor/offeror binding; exact stream sequence/head; all act/decision/event references bind | Session becomes `withdrawn`; cancel Offer-validity timer. Formation timer remains so the still-`negotiating` aggregate can expire unless a separately authorized cancellation is later added. |
| Offer-validity `TimerFired` | Host timer | No A202 event exists for mere Offer-validity expiry | Exact timer generation and scheduled time; current Offer still matches timer payload | Set `offer_acceptance_expired`; remove only the accept Action Offer. Session remains `active`, and a new Proposal Revision may still be legal. |
| Formation-deadline `TimerFired` | Host timer | No A202 signed event yet | Exact timer generation scheduled at the deadline; if already committed/cancelled/expired, consume as a deterministic no-op; otherwise aggregate is eligible pre-commit | Gate commercial Actions and record the next phase. The equality-time Timer does **not** itself satisfy A202’s strict “deadline passed/exceeded” event guard. Attention targets only the venue-signing Agent. |
| `record_session_deadline_elapsed` | Venue signer | Venue-signed Action Envelope, signed Policy Decision `allow`, and venue-signed session `deadline.elapsed` event whose data is exactly `{"close_reason":"session_expired"}` | Session is `active`; stream sequence/predecessor current; event `occurred_at` and WorldStream Action `admitted_at` are both strictly after the formation deadline; event time is not later than admission; all objects bind | Session becomes `expired`; set `transaction_expiry_pending`. |
| `record_transaction_deadline_elapsed` | Venue signer | A second venue-signed Action Envelope, signed Policy Decision `allow`, and venue-signed transaction `deadline.elapsed` event | Aggregate is `negotiating` or `agreement_pending`; transaction sequence/predecessor current; session is not `active`; signed event time and host admission time pass the same strict checks | Aggregate becomes `expired`; if session is `opened` or `accepted`, set `session_close_pending`; otherwise deadline resolution is complete. |
| `close_transaction_expired_session` | Venue signer | A third venue-signed Action Envelope, signed Policy Decision `allow`, and venue-signed session `session.closed` event whose data is exactly `{"close_reason":"transaction_expired"}` | Aggregate is `expired`; session is `opened` or `accepted`; session sequence/predecessor current | Session becomes `closed`; deadline resolution is complete. |

A202 says a counteroffer is a new immutable Offer, must supersede the immediately preceding Offer, and cannot be accepted after its validity time; withdrawal is allowed only before acceptance ([offer rules](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L198-L209)). Its session transitions separately define signed Offer submission, exact-hash approval, Acceptance, withdrawal, and deadline expiry ([session transition table](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L157-L186)).

The profile further closes each reference-shaped event payload: `offer.submitted` carries only `offer_id` and `supersedes_offer_id`; `offer.accepted` only `acceptance_id`; `offer.selected` only `offer_id` and `session_id`; `agreement.committed` only `agreement_id`; and transaction `deadline.elapsed` uses an empty `data` object. Terms never appear in events.

### Exact human approval

The first profile should approve the buyer’s **acceptance act**, not a vague UI summary:

1. The seller’s current Offer is already shared and has exact canonical bytes and content hash.
2. The buyer agent constructs the exact A202 Acceptance Action Envelope it wants to submit.
3. `request_exact_approval` records its `action_hash`, referenced Offer ID/hash, selected terms diff, and approval expiry.
4. The human sees those exact terms and signs an A202 Approval whose `action_hash` is identical.
5. The acting party’s evaluator reruns the unchanged Action Envelope with that Approval and current mandate/key status, then issues a fresh signed `allow` Policy Decision over the same `action_hash`.
6. `accept_current_proposal` succeeds only with the byte-identical candidate, fresh `allow`, and still-current Offer, Approval, A202 stream head, and WorldStream Head.

Changing one byte of the acceptance candidate, changing the Offer, letting either expire, or advancing the target A202 stream makes the approval unusable. A202 requires precisely that property: an Approval binds one exact action and transaction and cannot be replayed across changed actions or transactions; the official reference flow explicitly runs mandate verification again with the Approval and identical proposed Action to obtain `allow` ([Approval requirements](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/authority/commercial-mandate-v0.1.md#L290-L303), [reference approval flow](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/reference/a202_mcp/tools.py#L45-L61)).

The initial profile intentionally does not claim the A202 `paused_for_approval` event pair. It performs pre-approval before the Acceptance is appended and records the signed Approval object as evidence. This avoids a specification ambiguity: `approval.requested` and `approval.granted` are themselves session events, while an Action Envelope’s signed `expected_sequence` is supposed to name the target stream head. Advancing the stream for the approval events can stale the held Action Envelope the Approval bound. That question should be raised upstream before WorldStream claims those pause transitions.

The approval request and decision are session-visible in the first profile. Hidden canonical approval transitions would still move the single WorldStream Head and could be inferred through later Head gaps or stale-basis failures. Private approval thresholds and policy reasons stay outside the Room; the fact that a named human approved this exact shared Offer is evidence, not private strategy.

### Expiry and operator signing

WorldStream timers solve eligibility but cannot create an A202 signature. `ActivityPackV1` has no secret-key or external signing capability, and adding one would violate the current deterministic pack boundary. A202, meanwhile, says only signed authorized events move state ([A202 state-machine rule](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L9-L12)).

Offer validity and formation deadline are distinct. Expiry of `Offer.valid_until` makes only that Offer non-acceptable; A202 defines no event that closes the session merely because an Offer expired. The pack records that semantic fact from the Offer-validity timer and may continue to offer a counter/new Proposal Revision.

The profile pins one formation timestamp as both the A202 session deadline and transaction deadline. This makes the intended closure unambiguous without pretending one event changes both streams. Deadline resolution is state-sensitive:

1. The WorldStream formation Timer fires at its immutable scheduled time, removes commercial Action Offers, records the next required closure phase, and emits Attention only for the `venue_signer` Agent Participant. Commercial Actions admitted at or after their half-open formation deadline fail before reduction. The separately offered venue-closure Actions open at that boundary, but the Timer scheduled exactly there is only the gate—it is not evidence that A202’s clock strictly passed the deadline ([WorldStream eligibility](../crates/worldstream-core/src/activity_pack.rs#L413-L489)).
2. If the session is `active`, the signer first submits a venue-signed Action Envelope, fresh signed `allow` Policy Decision, and session `deadline.elapsed` event carrying exactly `{"close_reason":"session_expired"}`. Its signed `occurred_at` and the host-recorded Action `admitted_at` must both be strictly later than the deadline, and `occurred_at` may not be later than admission. Only that session stream advances to `expired`.
3. The signer then submits a distinct signed act/decision/event for transaction `deadline.elapsed` under the same strict time checks. Only the aggregate stream advances to `expired`.
4. If the session was `opened` or `accepted` at the deadline, session `deadline.elapsed` is not a legal transition. The aggregate expires first; a final separately signed `session.closed` event uses the already registered truthful reason `transaction_expired`.
5. If the session was already `withdrawn`, `expired`, or `closed`, only the eligible aggregate expiry is required. If the aggregate already committed or otherwise became terminal, the timer was cancelled; an already-admitted race is consumed deterministically without fabricating an A202 event.

Every listed event has its own Action Envelope, Policy Decision, expected sequence, predecessor hash, and signature. The terminal session events also carry the exact required closed `sessionCloseData`; no combined synthetic “expire everything” event exists. A202’s aggregate guard says the authoritative clock **exceeds** the deadline, while its session guard says the deadline **passed**, so equality is deliberately insufficient ([A202 deadline guards](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L104-L118), [terminal-data routing](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/v0.1/commercial-kernel.schema.json#L1923-L1957), [close-reason enum](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/v0.1/commercial-kernel.schema.json#L2146-L2166)).

If the signer is unavailable, late commercial acts remain impossible and the durable Activation Intent survives restart. The UI may show “deadline reached; signed closure pending.” Replay never invokes the signer. This uses the current distinction between durable logical agent participation and ephemeral external Invocations ([ADR 0009](adr/0009-activation-intents-context-and-lease-fencing.md)); no Room Kernel semantic change is needed.

## `ActivityPackV1` operation mapping

### `descriptor`

Declare:

- four Roles and their cardinalities;
- bounded wrapper schemas for every Action;
- Action types listed in the lifecycle table;
- stable domain rejection codes mapped to A202 refusal codes where an exact A202 code exists;
- Agent-only Attention reasons such as `proposal_received`, `venue_signature_required`, and `agreement_ready`; human approval notification is an addressed observation, not Attention;
- participant, operator, historical, public, and final-reveal projection/observation schemas;
- strict size/count/nesting/text limits.

The descriptor’s schema dialect is intentionally smaller than the full A202 JSON Schemas. Wrapper schema validation handles bounded transport shape; the exact pinned A202 validator runs deterministically inside `reduce`. That is pack implementation, not a new host-contract feature.

### `initialize`

Validate all protocol pins, Room configuration, initial Memberships, Role-to-kind constraints, identity/key/mandate/resolver trust references, transaction/session IDs, official profile schema, the shared formation deadline, and pre-opened event evidence. Produce the complete initial Activity State and initial timers.

### `reduce`

For participant Actions:

- identify the Membership and Role from immutable Core state;
- decode exact byte wrappers;
- verify A202 canonicalization, content hashes, signatures, signature purpose, mandates, policy decisions, exact approvals, object references, stream sequences, predecessor hashes, state-machine guards, profile terms, and deadlines;
- return a declared domain rejection for ordinary invalid/stale protocol acts;
- return `PackFault` only for an internal invariant or impossible trusted input;
- emit only the complete next state, ordered reference-shaped Domain Events, timer requests, and Agent-only Attention Signals. Approval visibility is represented in state/events for `observe`; `reduce` does not emit observation frames.

For timer stimuli, consume the exact Timer Generation and record the state-specific expiry phase. For Core membership proposals, accept only changes that preserve configured party/approver/venue bindings. For the profile’s predefined resolver-status `ExternalInputV1`, recheck the exact response/document bindings and deterministically update the recorded key/mandate/status view; because accepted External Input cannot be cleanly rejected, host authorization, origin validation, and source-specific schema validation must fail closed before pack entry. This is a narrow authenticated source, not a general connector ([Activity Pack external-input contract](activity-packs.md#reduction-input-and-normalized-stimulus)).

### `view`

Return one complete authorized current Projection plus the only canonical Action Offers. In particular:

- buyer and seller see transmitted Proposal Revisions, their diff, current A202 session state, deadline, and evidence status;
- the human approver sees the exact pending act and Proposal Revision it binds;
- the venue signer sees only the event candidate and evidence required to sign a venue event;
- the operator view gets bounded diagnostics, not a privacy bypass;
- spectators get an explicitly safe summary, never raw commercial terms by default;
- reservation values, prompts, hidden policy thresholds, private denials, and drafts are absent from every Projection.

### `observe`

Emit zero or one viewer-specific observation per accepted Transition. In particular, `observe`—not `reduce`—constructs the Human approver’s addressed pending-approval/decision frame. Other observations should carry object IDs, content hashes, term diffs safe for that viewer, session/aggregate state changes, approval status, integrity status, and changed Action Offers. They must not duplicate raw internal state or turn a hidden fact into a timeline leak.

## Privacy model

| Data | Buyer agent | Seller agent | Buyer approver | Venue signer | Operator | Spectator |
|---|---:|---:|---:|---:|---:|---:|
| Transmitted Proposal Revisions | Yes | Yes | Yes | Hash/reference as needed | Bounded | No by default |
| Current term diff | Yes | Yes | Yes | No | Bounded | No |
| Exact buyer acceptance Action Envelope | Yes | No; A202 keeps it actor-private even after the resulting Acceptance is shared | Yes | Hash/reference as needed | Bounded | No |
| Signed Approval after recorded | Yes | Yes in first profile | Yes | Reference | Bounded | No |
| Signed A202 stream events | Authorized streams | Authorized streams | Relevant subset | Event it signs | Bounded | No |
| Agreement after commitment | Yes | Yes | Yes | Hash/reference | Yes | Optional final reveal only |
| Reservation value / tactics / prompt / reasoning | Never stored | Never stored | Never stored | Never stored | Never stored | Never stored |
| Policy denial details | Actor-local response only | Actor-local response only | No | No | Operational code only | No |

A202 events carry references rather than terms, use closed event-data allowlists, and require per-stream evidence so that event records cannot become a rival-data side channel ([canonical event-stream rules](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/canonical-commercial-model-v0.1.md#L348-L418)). The first WorldStream profile has no rivals, but it should implement the same references-only discipline so a later extension does not have to remove leaked data from a durable format.

An ordinary invalid proposal should return an Activity rejection, not a Transition. WorldStream’s durable disposition consumes no Room sequence, while the rejected actor receives a stable result. That aligns with A202’s rule that a denied action consumes no shared-stream sequence and remains private to the actor ([A202 per-stream consequences](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L211-L230), [WorldStream Room Commit dispositions](adr/0006-backend-neutral-atomic-room-commit.md)).

## Persistence, reconnect, and Replay

No separate negotiation database should exist. The exact A202 bytes, logical stream heads, current Proposal Revision, approval, agreement, and evidence index are Activity State or recorded Stimulus/Event data bound by the Room lineage. The Room Commit atomically persists them with timers, observation frames, activation decisions, and the semantic receipt.

Reconnect is a WorldStream delivery concern:

1. Every durable buyer, seller, approver, and venue Membership has its own Observation Stream.
2. A reconnecting client presents its Cursor.
3. WorldStream delivers retained authorized frames or an explicit Projection Reset at one captured barrier.
4. The client becomes live only after the separate synchronization acknowledgment.
5. The agent starts a fresh Invocation from the bounded current projection; no “sleeping mind” or prompt transcript is restored.

A202 transport remains an adapter surface. Its A2A binding explicitly says carrier task state, connection failure, timeout, streaming, and retries do not change commercial state; commercial correlation comes from signed transaction identifiers, not carrier sessions ([A202 carrier purpose and correlation](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/bindings/a2a-binding-v0.1.md#L11-L21), [task/transaction separation](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/bindings/a2a-binding-v0.1.md#L151-L176)). A2A, REST, MCP, Python, and WebSocket ingress must all end in the same authenticated WorldStream Action path.

## Evidence model

The export must preserve two proofs without pretending they are the same:

### Party/protocol proof

- exact A202 canonical bytes for Action Envelopes, Policy Decisions, Offers, Approval, Acceptance, Agreement, and Transaction Events;
- exact A202 transaction/session stream sequences and predecessor hashes;
- referenced mandates, keys, status evidence, profile schema, and rules version;
- A202 SHA-256 content hashes and signatures;
- A202 verification report with `verified`, `failed`, and `not_checkable` kept distinct.

A202’s verifier recomputes canonical hashes, verifies signatures and purposes, checks object version chains and per-stream continuity, replays guarded transitions, and reports unresolved references rather than treating absence as success ([evidence procedure](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/evidence/evidence-verification-v0.1.md#L101-L190)).

### Venue/runtime proof

- Room ID, Genesis hash, Core schema version, exact pack digest, and initial protocol pins;
- ordered WorldStream Transitions and Complete Heads;
- cross-index from each accepted A202 object/event hash to the Room sequence whose Transition bound it;
- timer generation and scheduled time for expiry;
- exact approval Membership and Action receipt;
- final Activity/Authoritative state hashes;
- replay result under the retained exact pack executor.

The WorldStream proof establishes what this operator committed and in what Room order. The A202 proof establishes the signed commercial objects and logical stream histories. Neither alone proves legal enforceability, fair pricing, delivery, operator non-equivocation outside the disclosed lineage, or that an external key was controlled by the person its metadata names.

The evidence exporter and offline verifier are product-layer work. The canonical lineage already retains the necessary Stimulus/Event/state bytes, but a stable public export that carries exact A202 byte blobs and the cross-index does not exist merely because Replay exists.

## Where no Kernel semantic change is needed

The mapping does **not** require:

- another Room lifecycle;
- a second authoritative process;
- another sequence in WorldStream Core;
- a generic event bus;
- a remote reducer;
- model execution inside the server;
- cross-Room mutation;
- a mutable pack revision;
- a new Observation Stream contract;
- a new timer model;
- a private-key capability in `ActivityPackV1`.

It requires new artifacts above the existing seam:

1. A separately packaged negotiation Activity Pack revision.
2. A pinned A202 codec/schema/signature-validation library inside that pack.
3. A byte-preserving A202 ingress/egress adapter.
4. A configured identity/mandate/status resolver with a narrowly authorized predefined External Input source.
5. An external venue-signing agent or service with a narrowly authorized WorldStream Membership.
6. A human approval client that signs exact A202 Approval bytes.
7. A purpose-built deal-room UI.
8. An evidence exporter and offline combined verifier.
9. Conformance fixtures proving the selected subset and clearly reporting unimplemented scope.

These are significant implementation tasks, but they are not evidence that the Room Kernel abstraction is wrong.

## Where the profile stops fitting

### Multi-supplier operation

The mapping stops fitting one Room when one A202 transaction has multiple confidential supplier sessions. A202’s independent stream sequences exist specifically so a supplier cannot infer rival activity from a sequence advance or conflict. WorldStream’s exact Complete Head covers every canonical change in the Room. Separate viewer frames hide content, but they cannot make a stale Room basis uninfluenced by hidden transitions. The correct future shape is separate session Rooms plus a separately specified aggregate authority, not one multi-session Room.

### Full A202 conformance

The profile does not cover all families in either formal role scope. A202’s reference implementation is intentionally not an operator and provides no venue, session-ordering service, network server/client, or storage ([reference README](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/reference/README.md#L7-L22)), so WorldStream fills a real operational gap. But that gap does not waive A202’s conformance partition.

Before any formal claim, WorldStream must:

- pass applicable upstream fixtures through the actual pack/adapter path;
- publish an exact compatibility matrix by object family, transition, profile, transport, and role scope;
- obtain two separate grades if it claims both official scopes;
- preserve `not_checkable` rather than marketing partial evidence as verified;
- resolve the approval-sequence and timer/event-signing questions with upstream maintainers.

### Successful-session closure in pinned A202

The pinned A202 revision has a known internal completeness gap on the successful path. Its state machine permits `accepted` → `closed` on `session.closed` after aggregate commitment, but every currently allowed `sessionCloseData.close_reason` describes non-selection, withdrawal, expiry, cancellation, inactive authority, or failed qualification. None truthfully represents the winning accepted session. A202’s own A202-0020 document identifies exactly this contradiction and proposes `transaction_committed`, but it is explicitly a draft, informative proposal and says no schema, specification, fixture, or runner rule has adopted the value ([normative state transition](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/negotiation/pilot-transaction-state-machine-v0.1.md#L174-L184), [pinned close-reason enum](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/schemas/v0.1/commercial-kernel.schema.json#L2146-L2166), [draft A202-0020 problem and status](https://github.com/a202-protocol/a202/blob/fa85aa8b49bfe7b3f7ded487c98500a600e92e41/proposals/A202-0020-awarded-session-close-reason.md#L1-L21)).

This profile must not invent `transaction_committed`, reuse a false reason, or quietly accept a schema-invalid event. At pinned v0.1 it records the aggregate as `committed`, exposes the WorldStream successful Outcome from that aggregate fact, and leaves the exact A202 session at `accepted`. If A202 adopts a truthful close reason in a later immutable revision, a new pack revision may append the corresponding event. This is an upstream protocol-version limitation, not a Room Kernel semantic change.

### Signing and network resolution

Deterministic signature verification fits a pack. Secret-key signing and DID/status network resolution do not. They must stay external and their exact outputs must be recorded. If a design requires the pack to call a KMS, fetch a DID, query a mandate endpoint, or sign during `reduce`, that design does not fit `ActivityPackV1`; change the adapter flow, not the pack boundary.

## A2CN cross-check

### Why it maps more directly

A2CN defines exactly two agent parties, explicit turn ownership, a single per-session sequence, immutable retries by `message_id`, and Offer/Counteroffer/Acceptance/Rejection/Withdrawal messages ([overview and turn rules](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L312-L375), [offer ordering](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L957-L1055)). It separately specifies local timeout/grace behavior, a human-approval pause, a deterministic transaction record, and a structured terminal audit log ([timeouts](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L1495-L1560), [approval](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L2988-L3087), [transaction record](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L1574-L1704), [audit log](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L1705-L1819)).

| A2CN | WorldStream mapping |
|---|---|
| Initiator/responder | `buyer_agent` / `seller_agent` Roles, or neutral `initiator` / `responder` Roles in an A2CN-specific pack |
| `sequence_number` | Pack-owned logical session sequence; usually one accepted protocol message per Room Transition |
| Turn holder | Activity State plus viewer Action Offers |
| `message_id` retry | WorldStream Action ID/receipt plus a pack-level A2CN message-ID index |
| Offer/counteroffer | Proposal Revision Actions |
| Acceptance | Exact current-offer Action; terminal A2CN `COMPLETED` |
| Withdrawal | Either party’s terminal Action |
| `AWAITING_HUMAN_APPROVAL` | Pack phase, human Role, exact signed ApprovalReceipt, and an addressed human observation/UI notification |
| Round/session timeout | WorldStream timers |
| TransactionRecord | Deterministically generated terminal pack output |
| AuditLog | Evidence export derived from canonical messages and Room lineage |

A2CN approval is especially straightforward: entering/leaving `AWAITING_HUMAN_APPROVAL` does not increment its protocol `sequence_number`; the paused act takes the next sequence only when transmitted. A signed ApprovalReceipt binds the session, Offer hash, mandate, trusted approver, and expiry ([A2CN approval binding](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L2988-L3087)). A WorldStream human approval Transition may advance `room_seq` while leaving the pack-owned A2CN sequence unchanged.

### Why it is not the better operator protocol

The closer mechanical match hides larger semantic differences:

1. **Authority is bilateral/local.** A2CN says each party maintains a session object and treats its local state as authoritative for outbound decisions. It defines a sequence/timestamp/DID tie-breaker when those states disagree. WorldStream has one authoritative Room and no such competing state authorities ([A2CN session object](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L1330-L1403)).
2. **Timeouts are locally inferred.** A2CN includes a 30-second grace window and local-clock conflict handling. WorldStream records host-owned timer generations and scheduled semantic time; it will deterministically choose one Room result rather than reconcile two local results ([A2CN timeout rules](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L1510-L1558)).
3. **Streaming is explicitly out of scope.** A2CN v0.2 carries messages over HTTP and webhooks; “real-time streaming negotiations” is excluded. WorldStream’s live observation/reconnect layer is additive, not an A2CN requirement ([A2CN scope](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L170-L203)).
4. **Acceptance is terminal agreement.** A2CN Acceptance triggers a deterministic transaction record. A202 deliberately keeps acceptance, selection, and dual-signed agreement commitment separate ([A2CN Acceptance and record](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L1198-L1247), [transaction record](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L1574-L1704)).
5. **Authority assurance is weaker at Level 1.** Its declared mandate is expressly self-asserted; cryptographic DID VC authority begins in the stronger tier ([A2CN mandates](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/spec/a2cn-spec-v0.2.0.md#L578-L724)).

The official Python reference is also evidence of a different implementation priority. Its `SessionManager` stores active sessions in process memory, while the configurable `SessionStore` interface overwrites whole JSON session values; the FastAPI module’s configured store is documented for post-commitment state rather than the active manager ([SessionManager](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/reference-implementation/python/a2cn/session.py#L143-L203), [SessionStore](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/reference-implementation/python/a2cn/session_store.py#L1-L82), [server state](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/reference-implementation/python/a2cn/server.py#L64-L85)). Its message endpoint still marks DID-based in-session key verification as a v0.3 TODO ([server endpoint](https://github.com/A2CN-protocol/A2CN/blob/ff6305eda5b1bd545a0c83596e6419bfc2985b07/reference-implementation/python/a2cn/server.py#L460-L488)). These are implementation observations, not claims that the A2CN protocol is invalid.

### A2CN decision

Use A2CN in three ways:

- as the closest UX benchmark for an agent integrating a bilateral negotiation;
- as a message/state conformance comparison for Proposal Revisions, turn-taking, exact approval, acceptance, and export; and
- as a possible later adapter if users already speak A2CN.

Do not make it the fallback now. Switch only if the A202 prototype proves that a useful signed operated path cannot be made atomic without changing the Room Kernel, or if external adopters explicitly require A2CN interoperability. If speed alone were the objective, an A2CN-like demo would be simpler; the current objective is to prove WorldStream’s distinctive operator semantics.

## Decision matrix

| Criterion | A202 narrow profile | A2CN v0.2 |
|---|---:|---:|
| One-Room state-machine fit | Good for one session; poor for multi-session aggregate | Very good for one bilateral session |
| Explicit operator role | Yes | No; optional future custody only |
| Exact signed commercial objects | Strong | Strong protocol-act signatures |
| Exact human approval | Strong object binding; pause sequencing needs clarification | Direct ApprovalReceipt/pause mapping |
| Separate acceptance and commitment | Yes | No; Acceptance completes |
| Private multi-counterparty isolation | Explicit, but requires separate streams/Rooms | Out of scope; only two parties |
| Realtime/reconnect supplied by protocol | Carrier concern, so WorldStream composes naturally | Explicitly out of scope, so WorldStream is additive but changes product shape |
| Evidence verification | Detailed three-valued procedure and per-stream Replay | Deterministic TransactionRecord plus AuditLog |
| Alignment with WorldStream differentiation | High | Medium |
| Fastest implementation | Medium/low | High |
| Recommended role | Primary compatibility target | Benchmark and possible later adapter |

## Required prototype before implementation commitment

The research answer is “architecturally feasible,” not “integration complete.” The next decision should be backed by one throwaway prototype that proves five risky facts:

1. **Exact bytes:** One valid upstream annotated Offer, Approval, Acceptance, Agreement, and session event survive the WorldStream Action wrapper, Activity State, persistence, restart, export, and A202 verifier byte-for-byte.
2. **Signatures:** ES256 and EdDSA verification are deterministic inside a pack revision; no private key or network call crosses the pack seam.
3. **Dual sequence:** One transaction-stream event and several session-stream events maintain independent A202 heads while WorldStream advances one Room Head.
4. **Exact approval:** Mutating the approved candidate, current Offer, Approval, or either relevant head makes the acceptance fail safely.
5. **Timer signing:** Formation expiry removes late Actions immediately, activates the venue signer, and eventually appends each required, independently signed per-stream deadline/close event without Replay causing an external effect.

The prototype should use the official calibration-service profile and upstream conformance fixtures. It should not introduce AI-capacity terms, a general protocol abstraction, a dynamic pack loader, or a production UI.

### Stop or narrow if

- exact A202 bytes cannot be retained and exported without changing canonical history;
- the operator-signed event cannot be prepared before commit or supplied afterward without an unprovable gap;
- exact approval requires rewriting already signed bytes;
- one-session operation still requires independent authoritative streams the pack cannot model;
- the implementation must put a private signing key or network resolver inside `ActivityPackV1`;
- the adapter becomes a second service that can mutate negotiation state without an accepted WorldStream Stimulus; or
- formal A202 compatibility requires claiming fixture families the product does not implement.

## Final recommendation

Proceed with the **WorldStream A202 v0.1 Operated Single-Session Formation Compatibility Profile** as the protocol-mapping direction, subject to the exact-byte/signing prototype.

Keep the Room Kernel unchanged. Build the first negotiation pack as a separately packaged application of the runtime. Put protocol transport, DID/status resolution, control-plane signing, and UI above the pack. Make the pack the sole deterministic judge of what enters authoritative Room state.

Treat A2CN as the closest benchmark and a credible later adapter, not as a reason to abandon A202 or to recreate A2CN’s bilateral local-authority model inside WorldStream.

The honest public claim after the first implementation should be:

> WorldStream runs a pinned, single-session A202 compatibility profile and publishes the exact supported object, transition, and conformance-fixture matrix.

It should not yet be:

> WorldStream is A202 conformant.

That stronger sentence must be earned separately for each official A202 role scope.

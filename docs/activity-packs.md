# Activity Pack Design

## Status

This is the frozen trusted `ActivityPackV1` host contract and the normative Agent Heist v0.1 pack specification. Investigation Room remains the v0.2 boundary probe.

`ActivityPackV1` is a stable semantic contract for retained v0.1 Rooms, not a portable or sandboxed public plugin ABI. Trusted implementations are compiled into the release; dynamic loading, third-party upload, and a generic effect interface remain out of scope.

The host seam and executable-retention decision are accepted in [ADR 0010](adr/0010-activity-pack-v1-and-executable-replay-retention.md).

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

One room pins exactly one Activity Pack revision. Packs cannot call each other or mutate another room in v0.1 or v0.2.

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

## Package contents in the frozen releases

Each trusted built-in pack contains:

    activity/
    ├── manifest data
    ├── PackRevisionLockV1 and semantic digest
    ├── canonical Rust state and input types
    ├── initialization
    ├── reduction
    ├── view, Action Offer, and observation construction
    ├── timer and attention reason definitions
    ├── deterministic fixture data
    ├── state, stimulus, and output codecs
    ├── conformance and privacy tests
    └── first-party UI projection schemas

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

The explanatory version is for people. Only the semantic revision digest selects executable rules. A pack may retain immutable Genesis seat identities in Activity State, but MUST NOT persist a second mutable index of current Role ownership; reduction reads current assignments from the supplied Core view.

## ActivityPackV1

The complete trusted host seam is:

~~~rust
pub trait ActivityPackV1: Send + Sync + 'static {
    fn descriptor(&self) -> &'static PackRevisionDescriptorV1;

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

Every callback is pure, synchronous, bounded, and complete before persistence handoff. The pack receives no storage, network, filesystem, environment, scheduler, HostClock, database clock, Session, delivery, telemetry, Activation, Runner, model, wallet, secret, or artifact-byte capability. Same-process Rust remains trusted and is not a sandbox.

### DeterministicContextV1

The context exposes only canonical-value utilities and domain-separated labeled randomness derived from Room seed, exact pack digest, next Room sequence, label, and index. It exposes no ambient entropy or time. Integer and fixed-point operations are permitted; floating-point canonical state is forbidden.

For the same revision lock, Genesis input, Core inputs, timer view, and ordered normalized Stimuli, every operation MUST return byte-identical canonical outputs on every supported platform and storage profile.

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
  proposal_kind,
  requester_evidence,
  recorded_at
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

CoreProposed kinds are Join, Resume, AccessModeChange, RoleChange, Archive, Suspend, and Depart. Their exact state delta is represented only by Core before/proposed after. Host-normalized NoChange never enters the pack.

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
- RoleChange.

Archive, Suspend, and Depart are mandatory Core proposals. The pack must Apply them; attempting to Reject one is PackFault. A required admitted TimerFired or ExternalInput also cannot be cleanly rejected. Exact-Head, authority, integrity, policy, idempotency, and storage outcomes remain WorldStream concerns.

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

Every new due value MUST be strictly later than the causing Stimulus's semantic effective time: admitted_at for ParticipantAction, scheduled_for for TimerFired, and recorded_at for CoreProposed or ExternalInput. Equal or backward time, wrong generation, implicit replacement, conflicting requests, invalid payload/time, or overflow is PackFault. Replay repeats the same normalization from reconstructed timers without a clock or scheduler.

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

eligibility_window is null when no recorded window applies. Offers sort by the descriptor's action order; no alternative legality representation exists. The canonical Action Offer bytes computed for an exact Head and viewer are reused without reinterpretation in Projection Reset, Observation Frames, Invocation Context, and host Action pre-admission. An absent action type cannot reach reduce. Presence is necessary but does not guarantee acceptance of a payload-specific proposal.

ObserveInputV1 contains:

- Core and Activity before and after;
- normalized Stimulus;
- ordered Domain Events;
- exact Membership viewer;
- the exact after-view Projection and Action Offer bytes.

PackObservationV1 is one bounded canonical, viewer-authorized change value and carries the supplied after-view Action Offer bytes when the viewer's offers changed. observe returns zero or one result. If the complete authorized before/after view changes, None is PackFault; if it is unchanged, one bounded authorized notice is still allowed. A hidden Transition returns None for that viewer.

A Projection Reset calls view. Genesis creates no Observation Frame. Visibility removal, Session closure, frame sequencing and retention, Cursor movement, attach/reset barriers, and delivery remain host responsibilities.

WorldStream wraps the Activity Projection with authorized Core facts. The pack never exposes raw Activity State to any viewer, including operator Memberships. Replay and final reveal are explicit typed viewers, never an authorization bypass.

## Attention

AttentionSignalV1 names a target Membership, declared reason, priority, optional deadline, deduplication key, and the applicable exact Action Offer types. One Apply may emit at most one Attention Signal per target Membership.

The pack determines Attention canonically. The host separately checks current agent-participant eligibility, authority, policy, and integrity before creating operational Activation work. Replay reproduces Attention but performs no policy evaluation and creates no intent, offer, lease, Invocation, or Action authority.

## Bounds, faults, and containment

The descriptor's maxima cover canonical state, Domain Events, timer requests, Attention Signals, projections, observations, nesting, collections, and text bytes. The host schema-validates and canonicalizes every output before commit.

PackFault includes:

- callback panic where unwinding can be caught;
- malformed, noncanonical, undeclared, or oversized output;
- invalid next state, event, timer request, Attention, Projection, Action Offer, or observation;
- a mandatory Core veto;
- view/observation privacy-contract failure;
- deterministic disagreement during Recovery or Replay.

A PackFault before commit creates no Transition, timer change, Frame, Activation, or new receipt. Repeated deterministic failure on required input faults the Room; canonical hash disagreement quarantines it. OOM, aborting panic, and a permanently blocked trusted in-process callback cannot be preempted safely, so an external process supervisor is the v0.1 recovery boundary.

## PackRevisionLock and embedded registry

revision_digest is a build-computed semantic identity, not a pack-declared label and not a digest of platform-specific machine bytes. It is the digest of canonical PackRevisionLockV1:

- pack ID and explanatory version;
- host-contract version and canonical-codec version;
- descriptor/manifest bytes excluding the digest field;
- every schema ID and exact schema-content digest;
- deterministic static-data digests;
- pack-owned rule-source digest;
- deterministic dependency-lock digest.

Every behavior, legality, rejection, event, timer, Attention, projection, observation, or visibility change requires a new digest.

Each release embeds PackRegistryV1 mapping an exact digest to:

- the compiled executor;
- PackRevisionLockV1 and descriptor/schema bundle;
- state, configuration, Stimulus, disposition, event, timer, Attention, view, and observation codecs;
- golden-corpus digest;
- selectable_for_new_rooms;
- runnable_for_retained_rooms.

selectable_for_new_rooms implies runnable_for_retained_rooms. A digest may become non-selectable while remaining runnable, but every digest referenced by retained Room lineage MUST remain runnable for load, advance, view, observe, Recovery, and Replay. Retaining only decoders is insufficient.

Startup rejects digest/lock/descriptor collisions. Recovery never downloads or dynamically loads code. A referenced digest with a missing executor or codec makes the affected Room unavailable and prevents a verified restore or transfer from becoming ready.

There is no in-place Room upgrade. A Room's digest and canonical Activity bytes never change except through ordinary Transitions under that same digest. A new semantic revision creates a new Room. Removing retained execution support is a future explicit compatibility break with a defined export/purge policy, never a silent migration.

Retain for the Room lineage lifetime:

- exact revision digest, revision lock, descriptor/schema bytes, codecs, executor, and golden evidence;
- Genesis, ordered Transitions, normalized Stimuli, events, timer changes, Attention, and canonical hashes;
- semantic receipt identities/tombstones and immutable referenced-resource identities needed by retained lineage.

Snapshots, current materializations, indexes, caches, telemetry, pruned delivery payloads, and retired Invocation Context bytes are replaceable or bounded. Removing every snapshot must still permit initialization and exact full Replay.

The release compatibility manifest enumerates every bundled executor/codec and its golden evidence. Release upgrade, verified restore, and storage transfer gates fully Replay every retained digest rather than merely decoding old state.

## Public plugins

Not supported. ActivityPackV1 freezes the trusted semantic seam required by retained v0.1 Rooms; it does not promise Wasmtime, a Component Model ABI, signing, dynamic registry service, third-party renderer isolation, untrusted resource metering, or a marketplace. Those require a separate post-v0.2 decision.

## Presentation boundary

A pack publishes typed projection schemas and semantic labels. It does not ship executable frontend code.

v0.1 has a first-party Heist renderer plus generic state/timeline panels. v0.2 adds a first-party Investigation renderer. Both use the same authorized protocol.

The project deliberately does not freeze:

- ViewSpec;
- a dashboard builder;
- custom widget ABI;
- pack JavaScript;
- runtime LLM-generated layout;
- a renderer marketplace.

## Reference Activity A: Agent Heist

### Purpose

Agent Heist is the v0.1 reference Activity and uses pack schema v1. It is one ordinary ActivityPackV1 implementation with no Heist-specific Kernel primitive. It proves:

- private participant views;
- structured negotiation;
- one shared sealed-decision window with concurrent submissions serialized by Room order;
- timer-driven state changes;
- conflicting actions;
- targeted agent activation;
- disconnect and catch-up;
- crash recovery;
- deterministic replay.

It is not intended to be a rich game platform.

### Frozen configuration and seats

The canonical v0.1 configuration is:

~~~json
{
  "pack_id": "worldstream.agent-heist",
  "pack_schema": 1,
  "roles": ["navigator", "insider", "broker"],
  "briefing_duration_seconds": 30,
  "negotiation_duration_seconds": 90,
  "commitment_duration_seconds": 30,
  "commitment_reminder_seconds_before_deadline": 10,
  "result_duration_seconds": 20,
  "maximum_plans": 12,
  "maximum_open_offers_per_role": 4
}
~~~

There are exactly three participant seats, in canonical order: Navigator, Insider, Broker. Genesis binds each seat to one immutable Membership ID. A human or agent Principal may occupy a seat at creation, but the reference fixture uses three Agent Principals.

Current standing and Role still come from Core. Suspension or departure makes that Genesis seat missing. A missing seat never changes the majority denominator, transfers its private knowledge, accepts a replacement Membership, or permits Role reassignment. Heist rejects participant joins and Role transfers that would replace a fixed seat; spectator/operator Memberships remain ordinary Core access.

Navigator initially owns the route clue. Insider owns the entry-window clue. Broker owns the required-tool and extraction clues. No seat initially knows the full solution.

### Deterministic fixture selection

The Genesis DeterministicContextV1 helper uniform_index selects one fixture from the following table in row order using the label agent-heist/fixture/v1 and bound 3:

| Index | Fixture ID | Route | Entry window | Required tool | Extraction |
|---:|---|---|---|---|---|
| 0 | canal_shift | canal | late | disguise | van |
| 1 | service_window | service | early | thermal_key | boat |
| 2 | roof_signal | roof | middle | jammer | motorbike |

The selected fixture ID and hidden truth enter canonical Activity State. The deterministic fixture data also defines the finite clue IDs and allowed claim codes; Actions never carry arbitrary clue text.

### Canonical Activity State

Agent Heist canonical state contains exactly:

- phase, phase generation, phase start, and phase deadline;
- fixture identity and hidden fixture truth;
- the three immutable Genesis seat-to-Membership identities;
- clues plus inspection, disclosure, and public claim-code facts;
- bounded exchange offers and their statuses;
- plans, per-seat endorsements, and challenges;
- at most one immutable sealed commitment per seat;
- result acknowledgements;
- final Outcome and canonical explanation.

Maps and sets encode in role order navigator, insider, broker. Created entities encode by creation Room sequence and then ID. Plan IDs are Action IDs. Host timer rows/generations, Frames, Cursors, Sessions, Runners, Activation, policy, integrity, wall time, detection time, and commit time are not Activity State.

A plan is the canonical tuple:

~~~text
{
  plan_id,
  proposer_role,
  created_room_seq,
  route,
  entry_window,
  required_tool,
  extraction
}
~~~

plan_id is the proposing Action ID. Identical plan field tuples cannot be proposed twice; the first accepted tuple in Room order is canonical.

### Action schemas and predicates

Every Action requires a healthy active Room, an enabled participant occupying its immutable seat, the exact current Head, a matching exact Action Offer, strict payload schema, and ordinary durable idempotency.

Action Offers sort in this exact order:

1. inspect_clue;
2. publish_clue;
3. offer_exchange;
4. accept_exchange;
5. propose_plan;
6. endorse_plan;
7. challenge_plan;
8. commit_move;
9. acknowledge_result.

| Action | Canonical payload | Pack predicate |
|---|---|---|
| inspect_clue | {clue_id} | Briefing, Negotiation, or Commitment; caller owns the uninspected clue. |
| publish_clue | {clue_id, claim_code} | Negotiation; caller knows the clue; claim_code is one predefined code for it; clue is not already published. |
| offer_exchange | {recipient_role, offered_clue_id, consideration} | Negotiation; recipient is a distinct enabled seat; sender knows the offered clue; consideration is either {kind: clue_disclosure, clue_id} for a recipient-owned clue or {kind: plan_endorsement, plan_id}; sender has fewer than four open offers. |
| accept_exchange | {offer_id} | Negotiation; caller is the addressed recipient; offer is open; both disclosure/endorsement terms remain valid. All effects apply atomically. |
| propose_plan | {route, entry_window, required_tool, extraction} | Negotiation; fewer than twelve plans exist; tuple is new. |
| endorse_plan | {plan_id} | Negotiation; plan exists. It replaces the caller's prior public endorsement. |
| challenge_plan | {plan_id, reason} | Negotiation; plan exists; reason is route_conflict, timing_conflict, tool_conflict, or extraction_conflict; caller's disclosed knowledge proves it; the role/plan/reason tuple is new. |
| commit_move | {selected_plan_id, contribute_required_resource} | Commitment; plan exists; caller has no commitment; admitted_at is in the half-open phase window. The result is immutable and sealed. |
| acknowledge_result | {} | Result; caller has not acknowledged. It is presentation acknowledgement only. |

There is no fallback commitment field, arbitrary chat, hidden free text, binary yes/no vote, or pack-side participant credential.

The exact revision declares stable pack rejection codes for wrong phase, unavailable seat, clue ownership/knowledge, invalid claim code, invalid or bounded offer, missing/duplicate plan, unsupported challenge, prior commitment, and prior acknowledgement. Stale Head, current authority, integrity, schema, idempotency, and exact deadline admission remain Kernel dispositions rather than pack rejection codes.

### Six-phase timer machine

The only phase order is:

    Briefing → Negotiation → Commitment → Resolution → Result → Complete

Every phase entry is an accepted Transition and increments canonical phase generation. Phase generation is carried in timer payloads to fence obsolete phase work; it is distinct from the host-owned timer generation.

The pack uses logical timers phase_deadline, commitment_reminder, and resolve_now. The host assigns all timer generations.

1. Genesis enters Briefing at created_at with phase generation 1 and requests phase_deadline at D1 = created_at + 30 seconds.
2. Firing the exact D1 witness enters Negotiation and requests the next phase_deadline at D2 = D1 + 90 seconds.
3. Firing D2 enters Commitment, requests commitment_reminder at D3 - 10 seconds, and requests phase_deadline at D3 = D2 + 30 seconds.
4. The reminder emits required_action_deadline only to still-missing enabled Agent seats. It does not extend D3.
5. The third distinct commitment enters Resolution immediately, cancels the current commitment_reminder and phase_deadline witnesses, and requests resolve_now at successor(admitted_at), the minimum representable semantic tick strictly after the Action's admitted_at.
6. Otherwise firing the exact D3 witness enters Resolution with missing seats explicit, cancels the still-current reminder if necessary, and requests resolve_now at successor(D3).
7. Firing resolve_now freezes commitments, computes the Outcome, enters Result, and requests phase_deadline at D4 = resolve_now.scheduled_for + 20 seconds.
8. The third distinct result acknowledgement enters Complete early and cancels D4; otherwise firing D4 enters Complete.
9. Complete is terminal Activity State. Core Room Status remains active until authorized archive.

Every new schedule is strictly later than the causing semantic time. Early and deadline closure use the same reducer path and generation checks. A deadline race is ordered only by the bounded Room lane and durable commit: Action first cancels/fences the old timer; timer first closes the phase and the Action is no longer applicable. There is no reservation, rebasing, vote consensus, or timing tie-break.

### Strict-majority selection and five-check outcome

The denominator is always the three immutable Genesis seats:

| Commitments | Selection |
|---|---|
| zero or one | no strict majority |
| two for one plan | that plan |
| two split | no strict majority |
| three, 3-0 | unanimous plan |
| three, 2-1 | majority plan |
| three, 1-1-1 | no strict majority |

Two plans cannot each hold two of three commitments. No arrival-order, earliest-plan, plan-ID, or other majority tie-break exists. Missing seats never improve the denominator or synthesize a commitment.

No majority yields:

~~~json
{
  "outcome": "failure",
  "score": 0,
  "reason": "no_strict_majority"
}
~~~

The canonical explanation also includes aggregate vote counts and missing roles in canonical Role order.

A selected plan earns one point for each named Boolean check:

1. route matches;
2. entry_window matches;
3. required_tool matches;
4. extraction matches;
5. at least one commitment selecting that plan has contribute_required_resource = true.

Score 5 is success. Score 3 or 4 is partial_failure. Score 0, 1, or 2 is failure. The canonical explanation contains the selected plan, aggregate counts, ordered missing roles, all five named checks, score, reason, and Outcome. There is no LLM judge.

### Viewer-scoped privacy and reveal

| Viewer | Visible pack data |
|---|---|
| Spectator/public Membership | Phase/deadline, seat presence, public claim codes, plans, endorsements, challenges, commitment count, and aggregate Result once available. |
| Participant Membership | Public view plus that seat's authorized clues, addressed offers, own sealed commitment, and exact current Action Offers. |
| Operator Membership | Explicit operational diagnostics and aggregate counts only; never raw Activity State, fixture truth, clues, offers, or individual commitments. |
| Result | Selected plan, aggregate votes, five checks, score, reason, and Outcome are public; individual commitments and contributor identities remain sealed. |
| Complete final reveal | A currently authorized Room Membership may receive fixture truth, clues/exchanges, and individual commitments through the separately typed final-reveal view. |

Live and historical Replay views remain Membership-scoped. Historical Replay reconstructs the viewer at the requested sequence. Final-reveal Replay is separately labeled, exists only after Complete, and still requires current final-reveal authorization. Hidden-only changes yield no unauthorized observation. Genesis yields no Frame.

### Attention

The only declared Agent Heist reasons are:

- offer_received: a new offer is addressed to the target seat;
- endorsement_requested: a new plan is proposed to another enabled seat;
- commitment_opened: Commitment opens and the target seat has not committed;
- required_action_deadline: the reminder fires and the enabled target seat is still missing;
- round_result_available: Result opens for the target seat.

If one Transition would produce more than one reason for one target, retain exactly the first in this precedence:

    required_action_deadline
    commitment_opened
    offer_received
    endorsement_requested
    round_result_available

Human participants receive ordinary observations instead of Activation. Host policy enables the deliberately absent Broker demo path. Policy, intent creation, lease, Invocation, participant capability, and Cursor remain noncanonical host concerns; Replay reproduces Attention only.

### Deterministic fixture actors

The release fixture provides cooperative Navigator, cautious Insider, and withholding Broker actors. Candidate-plan ranking is identical and deterministic:

1. more fields matching the actor's currently known clues;
2. more current endorsements;
3. lower creation Room sequence;
4. lexicographically lower plan ID.

This ranking selects only an actor's proposed or committed plan; it never resolves the Room's majority.

The Navigator inspects/publishes its route clue and proposes the highest-ranked complete candidate. The Insider inspects its clue, challenges disclosed conflicts, and endorses/commits the highest-ranked consistent plan. The Broker's first Invocation exits before Commitment; after a fresh commitment_opened or required_action_deadline Activation, it ranks current plans, submits its separately authorized commitment, and contributes the required resource to the best known plan.

The canonical successful golden ends with a fully correct two-of-three or three-of-three majority and at least one supporting resource contribution.

### Golden corpus and checkpoints

Every PackRevisionLockV1 for Agent Heist binds a golden-corpus digest covering canonical configuration, Genesis output, state, events, normalized host-assigned timer changes, Attention, views/observations, Action Offers, and hashes at these checkpoints:

| Checkpoint | Required assertion |
|---|---|
| Genesis | Briefing generation 1, exact selected fixture, D1 request, no event/Attention/Frame/Activation. |
| D1 | Negotiation at D1 and D2 requested from D1. |
| D2 | Commitment at D2; reminder and D3 requested; commitment_opened targets exact missing enabled Agent seats. |
| Reminder | D3 unchanged; required_action_deadline targets only still-missing enabled Agent seats. |
| Third commitment | Both old Commitment timers cancel; Resolution enters; resolve_now is strictly later than admitted_at. |
| D3 without all seats | Missing seats remain explicit; Resolution enters; resolve_now is strictly later than D3. |
| resolve_now | Majority/scoring matrix produces exact Result explanation and D4. |
| Third acknowledgement or D4 | Complete enters exactly once and final reveal becomes eligible. |

The acceptance matrix MUST cover:

1. all six phases and generation-fenced early/deadline closure;
2. zero, one, 2-0, 1-1, 3-0, 2-1, and 1-1-1 commitment cases;
3. success 5/5, partial failure 3/5 and 4/5, failure 0-2/5, and no-majority failure;
4. exact deadline equality, stale Head, concurrent commitments, duplicate Actions/timers, and lost replies;
5. crash after commit/before Frame publication and crash after early close/before resolution;
6. fixed-cutoff restart catch-up and Replay from Genesis after deleting every snapshot;
7. durable Activation restart, claim retry, lease expiry/reclaim, and separation from participant Action/Cursor authority;
8. paired privacy fixtures for live view, Frame, catch-up, reset, UI/log, operator, historical Replay, and final reveal;
9. rejection of final reveal before Complete;
10. byte-identical canonical state, events, timer effects, Action Offers, explanations, and hashes on every supported platform and storage profile.

Replay folds Genesis through the exact typed Stimuli with the retained executor. It does not copy terminal state, shallow-hash selected fields, contact a Runner, or reconstruct Activation work.

The corrected executable prototype on branch prototype/agent-heist-recovery at commit 932c06c supports these semantics. It is logic evidence only; it does not prove production database/transport concurrency, authorization enforcement, power-loss recovery, browser behavior, queue bounds, performance, or cryptography.

## Reference Activity B: Investigation Room

### Purpose

Investigation Room validates that the same runtime supports serious human-agent work without becoming a workflow engine or adding core concepts.

The built-in fictional fixture is Cold Chain Incident. A shipment appears to have exceeded its safe temperature range. Evidence arrives over time, and a later timestamp correction changes how earlier sensor data should be interpreted.

There is no live web ingestion. Fixture inputs are immutable and replayable.

### Participants, access, and input

| Role | Kind in reference demo | Responsibility |
|---|---|---|
| Lead | Human | Assigns/reviews work and submits final brief |
| Timeline analyst | Agent | Establishes ordering and clock consistency |
| Evidence analyst | Agent | Extracts source-linked facts |
| Challenger | Agent | Tests claims and resolves verification requests |
| Evidence feed | Host stimulus | Releases immutable fixture evidence |

The human Lead uses the same action.submit protocol as the agents.

### Evidence fixture

The fixture includes content-addressed:

- shipment manifest and custody timestamps;
- primary temperature-sensor readings;
- refrigeration maintenance record;
- loading-dock witness statement;
- ambient weather record;
- corrected sensor clock-offset notice.

The deterministic answer key concludes that the apparent spike is best explained by a sensor removed for maintenance plus clock offset, not proven product-temperature excursion. The rubric still requires acknowledging the lack of a redundant product probe as uncertainty.

### Phases

- Intake: initial evidence is released and assigned.
- Analysis: participants publish facts and claims.
- Review: required claim challenges and verification are resolved.
- Brief: the Lead can submit once minimum evidence and review criteria are satisfied or the deadline opens.
- Closed: deterministic score and reveal projection are available.

These phases are domain rules inside one state machine. They are not user-authored workflow nodes.

### Canonical Activity State

State contains:

- case objective and phase;
- immutable evidence metadata and version graph;
- visibility and assignment per evidence item;
- bounded private draft records;
- published facts with exact evidence digests;
- claims and revisions;
- support, contradiction, and dependency edges;
- verification requests and dispositions;
- stale reasons caused by superseded evidence;
- participant deadlines and action-relevant domain facts;
- structured final brief;
- deterministic rubric result.

Artifact bytes remain in the generic content-addressed store. Canonical Activity State references digests and declared metadata.

### Typed actions

#### assign_evidence

Lead assigns an available evidence item to a role/member.

#### claim_evidence

An allowed analyst claims an unassigned item through a bounded evidence work reservation.

#### publish_fact

Publishes:

    bounded statement text
    exact evidence digests
    fact code/category
    confidence integer from 0 to 100
    optional time interval

The pack verifies structure and reference authorization, not natural-language truth.

#### flag_source

Records a typed source concern: timestamp, completeness, provenance, internal_conflict, or unsupported_format.

#### propose_claim

Publishes a conclusion candidate referencing current nonwithdrawn facts.

#### support_claim

Adds exact fact IDs and a bounded reason code.

#### challenge_claim

Adds contradictory fact IDs, reason code, and bounded explanatory text.

#### revise_claim

Creates a new immutable claim revision and marks the old revision superseded.

#### request_verification

Lead or analyst sends a named fact or claim to the Challenger with a deadline.

#### resolve_verification

Challenger records verified, rejected, insufficient_evidence, or stale with exact references.

#### submit_brief

Lead submits:

    conclusion_code
    ordered timeline entries with evidence references
    selected claim revisions
    contradictions addressed
    uncertainties
    overall confidence from 0 to 100

### Evidence correction and invalidation

When the corrected clock-offset notice arrives:

1. the host records the evidence-release stimulus;
2. the pack registers a new immutable evidence version and supersedes the old timestamp interpretation;
3. facts citing the superseded item are marked stale;
4. claims depending on those facts are marked stale;
5. verification results depending on them are marked needs_review;
6. domain events list exact invalidated IDs;
7. attention signals target affected Agent Participants and the active agent verifier; the human Lead receives an Observation Frame, while an agent-operated Lead could receive an Attention Signal.

No semantic vector search or LLM decides dependency. Explicit evidence and fact IDs make invalidation deterministic.

### Attention reasons

- evidence_assigned;
- verification_requested;
- cited_evidence_superseded;
- dependent_claim_stale;
- review_deadline;
- final_brief_opened.

The reference demo MUST use cited_evidence_superseded or dependent_claim_stale to start a fresh agent invocation.

### Deterministic scoring

The fixture rubric scores integers for:

- correct conclusion code;
- use of manifest, maintenance, sensor, and correction evidence;
- correct event ordering after applying clock offset;
- explicit treatment of the witness contradiction;
- withdrawal or revision of stale claims;
- acknowledgement of missing redundant measurement;
- required structured brief fields.

Text style is not scored. There is no LLM judge or hidden chain-of-thought requirement.

### Projections

Lead projection:

- published evidence metadata, facts, claims, challenges, reviews, deadlines, and brief requirements;
- no analyst private draft text unless explicitly published.

Analyst projection:

- assigned evidence and authorized artifact references;
- own drafts;
- published case board;
- own stale dependencies and Action Offers.

Challenger projection:

- authorized evidence;
- published board;
- assigned verification queue and deadlines.

Public/spectator projection:

- published case board and phase;
- no private evidence assignment or draft;
- final brief and score after closure.

Operator-membership projection:

- explicit operational diagnostics and authorized aggregate fixture status;
- never raw Activity State, private drafts/evidence, model chain-of-thought, or provider credentials.

### Investigation generality gate

The activity passes only if:

1. it uses the Activity Pack host interface frozen after Heist;
2. it adds no Investigation-specific wire message;
3. it adds no core domain table beyond generic artifact metadata;
4. it uses existing room ordering, timer, projection, cursor, activation, recovery, and replay semantics;
5. a human and agents submit actions through the same path;
6. a correction deterministically invalidates dependent work;
7. targeted activation supplies only authorized context;
8. the final brief and score replay identically;
9. Heist conformance remains green.

If these fail, the team must revise and retest the abstraction rather than hiding a special case in core.

## Activity conformance suite

Every built-in pack MUST pass:

- PackRevisionLock, descriptor, schema, codec, executor, and golden-corpus consistency;
- initialization determinism and no Genesis event, Attention, or Frame;
- golden Transition and final Activity State hashes;
- repeated reduce output equality;
- exact Core veto matrix plus participant rejection and Kernel stale-action conformance;
- wrong-generation, non-forward, duplicate/conflicting timer-request faults;
- maximum-state, collection, nesting, text, and output bounds;
- Action Offer byte parity across view, observation, reset, Invocation Context, and host pre-admission;
- projection/observation schema validation and zero-or-one viewer output;
- randomized cross-participant privacy/noninterference;
- completed-reveal authorization;
- activation-reason declaration and deduplication;
- callback panic and malformed-output containment before commit;
- crash recovery from an older snapshot and from Genesis with every snapshot deleted;
- Replay without external I/O, policy evaluation, scheduler, or Activation creation;
- retained non-selectable digest remains fully runnable through load, advance, view, and Replay;
- absence of floating-point authoritative values;
- no core changes specific to the pack.

## Future portable ABI decision

ActivityPackV1 is frozen for trusted retained Rooms. After v0.2, use the two real implementations to decide whether a separate portable/untrusted ABI is justified:

- whether the interface should remain a Rust crate API;
- whether a WebAssembly Component Model boundary is justified;
- which schema/version compatibility rules are real;
- how resource limits and deterministic execution are enforced;
- whether third-party packs are worth the security and support cost.

No marketplace or public upload flow should be built merely because an Activity Pack host interface exists.

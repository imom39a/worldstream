# Late joining, current state, and bounded agent context

Status: Non-normative worked example, 2026-09-12. Existing contracts are
identified below; the airline Pack, its limits, and historical retrieval
adapter are illustrative proposals, not implemented features. This document
does not change the five-operation Pack contract or accepted ADRs.

## What we already decided

A late-joining agent does not consume a full Replay. A first attachment gets
a **Projection Reset**: a complete current view authorized for its Membership,
including its Action Offers. A returning Membership can receive retained
Observation Frames after its Cursor, or a Reset when that range is unavailable
or inappropriate. The synchronization barrier bridges this baseline to live
delivery without a handoff gap. Session synchronization and observation
acknowledgement are separate operations.

These are accepted semantics in
[ADR 0008](adr/0008-membership-observation-streams-and-reset-barriers.md).
**Replay** instead reconstructs canonical history for recovery, verification,
or an explicitly authorized historical view. It is not a conversation that
an agent must read before participating.

Activation preparation reuses an installed serving trace. After a restart, both storage adapters can recover from a verified checkpoint with at most 250 tail Transitions. Missing or untrusted checkpoints require full Genesis Replay.
The model still receives a bounded current Projection rather than recovery
history. Operational-witness capture and verification scale with the retained operational rows. A canonical witness above 16 MiB causes a fallback.

A new Membership does not inherit another Membership's Cursor, private
observations, or historical visibility. Its current Projection follows the
Pack's present visibility rules. The historical authorization boundary is
specified in [ADR 0005](adr/0005-canonical-core-state-integrity-and-hash-lineage.md).

## The data has different owners and lifetimes

| Data | Maintained by | Lifetime and growth |
| --- | --- | --- |
| Genesis and accepted Transitions | WorldStream canonical storage | Retained for the Room lineage lifetime; grows with history |
| Core Room State | Core reducer | Room Status and semantic Membership map only |
| Activity State | Pack reducer, persisted by WorldStream | Bounded current domain facts; replace obsolete values and retire resolved work through accepted rules |
| Current materializations and recovery snapshots | WorldStream storage | Rebuildable caches bound to canonical lineage; snapshots are not agent summaries |
| Projection and Action Offers | Pack `view`, wrapped by WorldStream | Complete authorized current view, with explicit output bounds |
| Observation Frames and Cursor | WorldStream | Bounded retained delivery stream plus durable processing position |
| Activation claims and retained Invocation Context | WorldStream operational state | Execution fencing, receipts, and bounded context retention |
| Prompt history and Agent-Private Memory | External Runner | Separate bounded context and persistence policies |

The supported SQLite/PostgreSQL profiles persist canonical and operational
records under the Room Commit contract. The Pack does not open a database or
maintain its own authoritative store. Current rows are verified materializations
of the canonical lineage, not an independently writable truth.

An accepted change atomically records its Transition and required state,
receipt, timer, observation, and Activation consequences. Updating a current
fact does not delete its earlier canonical history. The current V1 Transition
format also embeds resulting state, so keeping Activity State small matters
for disk usage as well as context. A more compact canonical format remains a
separate proposal.

The retention contract is in [Activity Pack retention](activity-packs.md#packrevisionlock-and-startup-registry).
The [atomic commit decision](adr/0006-backend-neutral-atomic-room-commit.md)
governs persistence. Transport retention is not a model token budget.

## A concrete view of one changing situation

Suppose a Room has reached sequence 100,000 while tracking a small set of
flight connections. A new eligible analyst attaches. The following is an
illustrative **`projection.activity`** value, not the full protocol envelope:

```json
{
  "phase": "monitoring",
  "objective": "Identify connections needing intervention",
  "connections": [
    {
      "connection_id": "C17",
      "arrival": { "source_version": 18, "eta": "2026-09-12T15:20:00Z" },
      "departure": { "source_version": 9, "closes_at": "2026-09-12T15:35:00Z" },
      "minimum_transfer_minutes": 25
    }
  ],
  "open_work": [
    {
      "work_id": "W4",
      "revision": 3,
      "connection_id": "C17",
      "status": "needs_assessment",
      "reason": "arrival_changed",
      "last_assessment": {
        "claim": "connection_feasible",
        "validity": "superseded",
        "arrival_version": 17,
        "departure_version": 9
      }
    }
  ]
}
```

The surrounding protocol supplies the Room Head, integrity, Membership,
schema, and baseline metadata. The real Projection shape is
`{ core, activity, action_offers }`. Its separate Action Offer list might
contain this illustrative entry; the schema digest is symbolic:

```json
{
  "domain": "worldstream/action-offer/v1",
  "action_type": "record_assessment",
  "payload_schema_digest": "blake3:<exact-payload-schema-digest>",
  "eligibility_window": null
}
```

The current Action Offer does not contain a prompt, a recommended payload,
or every valid argument combination. It identifies the offered Action type,
exact payload schema, and optional eligibility window. The Projection
supplies work IDs and current facts. Payload relationships are checked in
`reduce`. Being offered is necessary, not a guarantee of acceptance.
See the actual [protocol structures](../crates/worldstream-protocol/src/messages.rs)
and [Action Offer contract](activity-packs.md#view-action-offers-and-observation).

The rule brief lets the analyst identify the open assessment and why the old one is obsolete. The analyst can submit this Action:

```json
{
  "room_id": "trip-room-1",
  "member_id": "analyst-2",
  "action_id": "assessment-attempt-48",
  "based_on_room_seq": 100000,
  "action_type": "record_assessment",
  "payload": {
    "work_id": "W4",
    "expected_work_revision": 3,
    "arrival_version": 18,
    "departure_version": 9,
    "claim": "connection_at_risk",
    "reason_code": "transfer_time_insufficient"
  }
}
```

The payload belongs to this proposed Pack; the surrounding ActionSubmit fields
are current. Its whole-Head basis must still be current at admission. Pack
source versions provide additional validation and provenance, not an escape
from that rule. A recorded assessment is an attributed conclusion; confirmed
external interventions require separate verified outcomes.

## What a developer writes in the Pack

The five existing operations divide the work:

| Operation | Example responsibility |
| --- | --- |
| `descriptor` | Declare Roles, Action/input schemas, observation schemas, attention reasons, and hard limits |
| `initialize` | Establish bounded initial source facts and current work |
| `reduce` | Validate one candidate, update current facts/work, and emit events, timer requests, and Attention Signals |
| `view` | Construct the complete authorized situation and exact current Action Offers |
| `observe` | Describe an authorized change, including changed offers; return no result only when the view permits that |

Conceptual reducer logic, not compilable SDK code:

```text
on a recorded, schema-valid source update:
    reject an obsolete source revision under the Pack's ordering rules
    replace the current source value
    for affected admitted work:
        advance its work revision
        mark any previous assessment superseded
        preserve or reopen the assessment obligation
    signal eligible agents that an assessment is needed

on record_assessment:
    check current participant Role and task eligibility
    check exact work revision and source revisions
    check payload relationships and the declared assessment rule
    store the attributed current assessment
    resolve this assessment obligation only if its criteria hold
    emit the domain consequence
```

Both source-driven work creation and participant-driven changes must respect
the configured active-work capacity. If capacity is exhausted, apply an
explicit bounded policy; never silently lose a required assessment. General
feed ingress is an integration still to build. The Pack itself cannot call
an airline API, model, database, or historical log.

This fits the existing five callbacks. The actual Midnight Archive Pack
already follows the same continuity principle: its
[companion Projection](../examples/packs/midnight-archive/src/view.ts) includes task,
plan progress, deadline, knowledge, preparation, and last contribution.
Its [Pack callbacks](../examples/packs/midnight-archive/src/pack.ts) compute the view
from current state. These are existing code examples, not an implemented
airline activity.

## How a generic agent understands the Pack

Schemas explain structure; they are not sufficient teaching material for an
unfamiliar activity. Supply a short, versioned rule brief through the Runner integration or documentation. Include the goal, Role, fields, Actions, evidence requirements, and completion/blocking criteria.
The SDK also supports schema descriptions. There is no new `explain` callback
or arbitrary brief field in ActionOfferV1 in this proposal.

At execution time the Runner combines that brief with the current Projection,
offered Action schemas, a bounded reason for activation, and any evidence
actually needed. The model chooses among legal contributions; the Pack does
not need to prescribe the uniquely optimal next action. If the current view
cannot explain a necessary constraint, expose an authorized fact or require
an explicit evidence-gathering step instead of expecting transcript recall.

## Preventing context bloat

Use a **replaceable current view**, not an ever-growing transcript of views.
Midnight Archive currently emits the complete after-Projection as its
observation payload. A compatible client can replace that view; appending
every such payload to a model conversation would duplicate state repeatedly.
Other Packs can define bounded change observations with explicit application
semantics. WorldStream does not make arbitrary observation JSON a universal
patch format.

For this proposed example, start with at most 16 open work records, bounded
source facts, one current/last assessment per work record, short reason codes,
and a 24-KiB Projection limit. These are proposed application limits, not
current global defaults. Enforce encoded byte limits too: cardinality alone
does not bound variable text. All required authorized unfinished work must
fit; do not return the top 16 while concealing the rest.

The Runner additionally enforces a total token budget across instructions,
schemas, Projection, recent outcomes, retrieved evidence, and reserved output.
A byte ceiling cannot be treated as an exact token ceiling. A transport-safe
256-KiB view may still be a poor model prompt. If essential current information
cannot fit the configured budgets, the Pack/integration needs a smaller
active scope or an explicit bounded interaction design.

Large completed details stay in retained canonical history or explicitly
supported immutable external evidence. Authorized historical navigation is
optional work with separate bounded pages, not automatic catch-up of every
past Transition. A general evidence query adapter is still proposed; current
Pack `view` has no arbitrary paging/query argument, and the Pack has no
storage access. Such an adapter cannot expose private history to new Members
merely because it is useful.

While an agent is offline, changes can invalidate work. Its replacement gets
the new current obligation and current inputs. It need not consume all of the
intermediate estimates to see that an assessment is required. When past detail
is itself necessary for a decision, retain the corresponding current fact or
provide an explicit authorized evidence path. A lossy summary cannot establish
that requirement by itself.

## Qualification criteria

- At the same bounded active-state size, joining after 100,000 Transitions
  supplies the same class and bound of context as joining after 100.
- A fresh Invocation with no private memory can identify every authorized open
  obligation and its current inputs, or explicitly identify missing evidence.
- Reset followed by live changes converges to the exact authorized `view` at
  the same Head under the Pack's observation semantics.
- Visibility changes and new Memberships disclose no inherited private history.
- A lost Action reply resolves to its original disposition; revised work cannot
  be completed by an old proposal.
- Required work is retained across retirement of delivery payloads and Runner
  contexts; limits cause explicit capacity handling rather than silent loss.

These are proposed checks, not test results from this document. Existing
server-scale gaps and the broader workload remain in the
[long-running primitives proposal](https://github.com/imom39a/worldstream/blob/ac7f443562bce33088229351d5f9eba40bb67bdd/docs/long-running-room-primitives-proposal.md).

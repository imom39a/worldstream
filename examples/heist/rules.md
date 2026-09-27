# Historical Heist rules

Retained design reference for the embedded conformance Pack.

## Demo/conformance Activity: Agent Heist

### Purpose

Agent Heist is a retained visual demo/conformance Activity and one ordinary ActivityPackV1 implementation with no Heist-specific Kernel primitive. It proves:

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

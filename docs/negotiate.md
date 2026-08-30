# WorldStream Negotiate

## Status and authority

This document is the normative first-release Activity Pack and integration
contract for `worldstream.negotiate`. It applies the unchanged Room Kernel and
five-operation `ActivityPackV1` contract to a pinned A202 commercial formation
profile. [ADR 0015](adr/0015-a202-operated-single-session-formation-profile.md)
owns the compatibility decision; this document owns the exact Pack behavior,
privacy, adapter, and evidence boundaries.

Counter is an internal tutorial/conformance fixture. Agent Heist is a visual
demo/conformance Pack. Investigation Room is deferred. Negotiate is the first
serious public Pack and MUST be built and proven through the public TypeScript
Pack Author path as a WASI-free Component.

## Compatibility statement

The only permitted public claim is:

> WorldStream runs a pinned, single-session A202 compatibility profile and
> publishes the exact supported object, transition, and conformance-fixture
> matrix.

WorldStream MUST NOT claim A202 conformance.

Independent-peer qualification is a separate release obligation from local
adapter conformance. The exact release invitation, retry vectors, and
candidate-receipt boundary are defined in
[A202 independent-peer interoperability](a202-peer-interoperability.md).

The **WorldStream A202 v0.1 Operated Single-Session Formation Compatibility
Profile** pins:

- A202 repository revision `fa85aa8b49bfe7b3f7ded487c98500a600e92e41`;
- shared object model `a202-commercial/0.1`;
- state-machine rules `1.3`;
- operated scope `a202-scope/operated/0.1`;
- required bilateral behavior from `a202-scope/bilateral/0.1`;
- upstream profile `a202-profile/calibration-service/0.1`; and
- exactly one transaction, one bilateral session, two commercial
  organizations, and one WorldStream Room.

The supported lifecycle begins with an imported, already-qualified and signed
opening history through `request.published`, `qualification.started`, and
`negotiation.opened`. It supports signed Proposal Revisions/counters,
author-only withdrawal, validity and formation expiry, exact Human approval of
the buyer acceptance act, acceptance of the exact current seller Offer,
operated selection, independent party signatures over one Agreement content
hash, and `agreement.committed`.

The successful Outcome is established at aggregate `committed`. The pinned
A202 session remains `accepted`; the Pack MUST NOT invent or misuse a
successful `session.closed` reason.

Publication/discovery, invitation, qualification, auctions, multiple
counterparty sessions, competitive award, cross-Room transaction aggregation,
obligations, performance, settlement, disputes, determinations, amendments,
termination, and operator custody of commercial-party signing keys are outside
the profile. A2CN is a later adapter/UX benchmark, not the first authority
model.

## Genesis

Genesis configuration contains only bounded authoritative facts:

- the exact compatibility-profile pins and transaction/session identifiers;
- the exact signed opening history;
- one shared formation deadline;
- party, approver, and venue public identities;
- mandate, public-key, and resolver bindings;
- one authorized resolver Host Stimulus Source;
- disclosure policy; and
- revision-bound resource limits.

Genesis MUST NOT contain private keys, credentials, prompts, model settings,
tactics, reservation values, mutable connectors, or unrecorded network
references.

Exactly four Participant Memberships exist at Genesis:

| Role | Principal kind | Cardinality | Responsibility |
|---|---|---:|---|
| `buyer_agent` | Agent | exactly one | buyer protocol acts and Agreement signature |
| `seller_agent` | Agent | exactly one | seller protocol acts and Agreement signature |
| `buyer_approver` | Human | exactly one | byte-exact approval or rejection |
| `venue_signer` | Agent | exactly one | signed deadline-resolution events |

Spectator and Operator are Access Modes, not Roles. Later Core changes MUST NOT
violate the Role/kind/cardinality or pinned A202 identity bindings. Suspension
or departure never fabricates a protocol event.

## Activity State and phases

Bounded Activity State contains:

- protocol/profile pins and identity/mandate/key bindings;
- aggregate and session states with independent logical heads;
- the exact current Proposal Revision bytes, hash, author, and validity;
- a pending exact acceptance candidate and its Approval, if any;
- Agreement content hash and independently recorded party signatures;
- exact A202 bytes indexed by object identity and hash;
- recorded resolver evidence;
- deadline progress; and
- the final Outcome, if established.

It MUST NOT contain drafts, prompts, reasoning, strategy, reservation values,
credentials, private keys, or a second Role index.

Activity Phases are exactly:

1. `formation_open`
2. `approval_pending`
3. `offer_accepted`
4. `agreement_pending`
5. `withdrawn_pending_expiry`
6. `deadline_resolution`
7. `complete`
8. `expired`

These phases are separate from A202 state, Room Status, and Outcome.

## Actions and Action Offers

The Pack declares exactly these Actions:

1. `submit_proposal_revision`
2. `withdraw_live_proposal`
3. `request_exact_approval`
4. `record_exact_approval`
5. `accept_current_proposal`
6. `select_accepted_proposal`
7. `record_agreement_signature`
8. `commit_agreement`
9. `record_session_deadline_elapsed`
10. `record_transaction_deadline_elapsed`
11. `close_transaction_expired_session`

Protocol-bearing Actions wrap exact canonical A202 bytes without
reserialization. Agreement signatures are collected independently over one
content hash. Commitment requires the assembled dual-signed Agreement and the
byte-identical signatures already verified by the Pack.

Only `view` determines Action Offers:

- either commercial party may create the first Proposal Revision;
- only the current offeree may counter and supersede its immediate predecessor;
- only the current offeror may withdraw before acceptance;
- the buyer may request approval only for a live seller-authored revision;
- only `buyer_approver` may approve or reject the exact candidate;
- only the buyer may submit the unchanged approved acceptance and select it;
- each commercial agent may record only its own Agreement signature;
- the buyer may commit only after both signatures are present;
- only `venue_signer` sees deadline-resolution Actions; and
- terminal phases offer no ordinary Actions.

A counter, withdrawal, relevant A202-head change, expiry, or byte mutation
invalidates pending approval. A Room Head change that leaves the A202
predecessor current permits resubmission of the same signed A202 bytes under a
fresh WorldStream Action basis. An A202 predecessor change requires rebuild and
re-sign; neither host nor client may auto-rebase signed protocol content.

## Exact approval, timers, and Attention

Approval binds the byte-exact proposed buyer Acceptance Action Envelope,
transaction, Proposal Revision hash, session head, approver, decision, and
expiry. Acceptance MUST submit the identical candidate with the signed
Approval and a fresh signed allow Policy Decision. A changed byte, Offer,
logical head, mandate/status evidence, or expired deadline rejects with no
Transition.

Timers cover current Proposal validity, pending approval expiry, and the shared
formation deadline. Formation expiry gates ordinary Actions, enters
`deadline_resolution`, and emits Attention for the external venue signer. The
Pack never signs an A202 object.

Attention reasons are exactly:

- `proposal_received`;
- `agreement_signature_required`;
- `agreement_ready`; and
- `venue_signature_required`.

Human approval is delivered through the approver's addressed Observation
Stream rather than Agent-only Attention.

## Outcomes

The only Outcomes are:

- `agreement_committed { transaction_id, agreement_id, agreement_hash }`; and
- `formation_expired { transaction_id, final_session_state }`.

Success requires aggregate `committed`. Room Status remains active until an
explicit archive Stimulus.

## Privacy matrix

| Viewer | Authorized information |
|---|---|
| buyer/seller | transmitted Proposal Revisions, authorized diffs, shared protocol state, deadlines, and evidence status |
| buyer approver | only the exact pending Acceptance/Proposal binding needed to decide |
| venue signer | only its event candidate and required resolver/protocol evidence |
| spectator | status summary with no commercial terms by default |
| operator membership | bounded diagnostics; never raw Activity State or an authorization bypass |

The buyer-private Acceptance Envelope is not disclosed to the seller.
Agreement bytes are limited to commercial parties, the approver, and an
explicitly authorized operator projection after commitment. Prompt, tactic,
reservation value, hidden threshold, reasoning, credential, and private denial
data never enter unauthorized Projections, observations, logs, or exports.

## Authority and adapter boundary

- `worldstreamd` is the sole venue authority for authentication, Membership,
  Room Head concurrency, Action Offers, accepted Transitions, receipts,
  persistence, Outcome, Recovery, and Replay.
- The Pack owns deterministic A202 validation: exact bytes/hashes, protocol
  signatures, purposes, mandates, exact approval, resolver evidence, logical
  sequence, deadlines, and profile rules.
- Buyer and seller Runners own strategy, prompts, memory, models/tools,
  credentials, policy, and private signing keys.
- The Human client renders and signs exactly the offered approval candidate.
- The venue signer remains an external Runner.
- A resolver is an authenticated Host Stimulus Source. The host validates its
  source/authentication; the Pack validates the recorded semantic binding.

A202 objects cross the adapter as opaque exact bytes, media type, and BLAKE3
digest. Their logical expected sequence remains inside the signed bytes; the
WorldStream Room Head is the separate venue concurrency witness. Resolver
evidence records exact response bytes, authenticated origin, observation time,
subject, digest, and host validity result.

There is no protocol side database, Pack network/key access, UI state mutation,
boolean-only approval, signed-object reserialization, hash-as-signature claim,
or projection-only audit source.

## Evidence export

One proof package contains two linked sections:

1. **Party/protocol proof:** exact A202 bytes, hashes, signatures, mandates,
   approvals, logical transaction/session streams, deadlines, and protocol
   status. Every verification is `verified`, `failed`, or `not_checkable`; a
   digest is never presented as a signature.
2. **Venue/runtime proof:** Room and Genesis identity, exact Pack Revision Lock
   and Activity Pack Bundle digest, Heads, accepted Transitions, timers,
   receipts, actor/source bindings, Outcome, Replay result, and cross-indexes
   from each A202 object to its admitting WorldStream Action/Transition.

A separate offline verifier consumes this export and the referenced Activity
Pack Bundle. UI projections display evidence but are never its source.

## Mandatory fixtures

The golden success path is:

`buyer Proposal → seller counter → exact approval request → Human signed
approval → persisted restart/reconnect → buyer acceptance → buyer selection →
two independent Agreement signatures → dual-signed commitment → identical
Replay and evidence cross-index`.

Negative fixtures MUST cover one mutated approved byte, stale Room Head, changed
A202 head, expired validity, wrong signer, Role-authority violation, private
field noninterference, signed formation expiry, forbidden imports, resource
exhaustion, callback fault before commit, and missing/corrupt retained bundle.
The success and negative corpus MUST pass both a native first-party oracle and
the public TypeScript Component path byte-for-byte.

The independent native specification lives in
`crates/worldstream-negotiate-oracle`. Its checked-in
`fixtures/corpus-v1.json` is the cross-language input/output boundary for the
TypeScript implementation. The crate has no `worldstream-core` dependency,
performs no runtime I/O, and MUST NOT be used as a production Pack executor.
Its deterministic fixture proofs bind exact bytes, signer, and purpose but do
not replace the pinned A202 schemas, SHA-256 rules, production signature
algorithms, mandates, or resolver checks.

## Participant Console integration

The production Console selects the Negotiate renderer only when the retained
Room identifies `worldstream.negotiate`. Its pack-specific bootstrap is a
one-shot, bounded `worldstream.negotiate.console.v1` document containing only
the authorized projection, exact Action Offers, Room sequence, persona, and
Replay/evidence availability. The browser deletes that bootstrap before React
renders. A separate ordinary WorldStream live-session bootstrap owns the
short-lived Membership authority and is consumed separately, then required to
match the Negotiate Room and Membership identities before a live renderer is
created. A mismatch fails closed without enabling Actions.

The live renderer uses the same protocol attach, Projection Reset or retained
Frame catch-up, `room.sync_ack`, exact-head Action, receipt, reconnect, and
Replay boundaries as every Participant client. It disables Actions until the
sync barrier is acknowledged, rejects cross-Room or cross-Membership traffic,
and refuses a Room whose pinned Pack ID is not `worldstream.negotiate`.
After that barrier it acknowledges delivered Frames with `observation.ack`, so
the durable Membership Cursor—not browser memory—defines reconnect catch-up.
Projection updates are installed only through the authorized `activity`
projection and server-supplied Action Offers; Core authority details and raw
Activity State never enter the renderer.

Prepared commercial payloads cross one narrow application-owned signer seam:

1. the Console emits `worldstream:negotiate-action-requested` with a fresh
   preparation request ID, Action type, payload-schema digest, Room/Member
   identity, and current Room sequence;
2. an independently controlled application or Runner prepares and signs the
   exact A202 payload without giving WorldStream its key; and
3. it returns `worldstream:negotiate-prepared-action` with exactly the request
   ID, Action type, payload-schema digest, preparation Room sequence, and JSON
   payload; the Console submits only if all four binding fields and the
   original exact Action Offer remain current.

This event pair is a browser integration boundary, not a second authority.
The normal transport bounds, server Action admission, Pack signature/head/
deadline validation, and typed receipt remain decisive. A stale Room Head
closes the submit gate; a changed A202 logical head additionally requires the
external signer to rebuild and re-sign rather than replaying or auto-rebasing
bytes.

The Studio human-seat handoff retains Room, Membership, and bearer authority in
an HttpOnly local Supervisor session. Its observation includes only the pinned
Pack identity needed to select this renderer. For a Negotiate Pack the Console
renders the same exact approval view and Replay hashes, while the
`worldstream:negotiate-retained-action-requested` signer event binds request,
Action, schema, Room sequence, and the opaque retained-session authority mode;
it never exposes Room ID, Membership ID, bearer, or routing data to the page.

Six DOM/API personas are tested independently: buyer agent, seller agent,
Human buyer approver, venue signer, operator, and spectator. The operator and
spectator renderings are invariant under injected private commercial fields,
and exact acceptance candidate bytes render only for the authorized Human
approver. Signature proof, bearer, API key, and server error detail never enter
the DOM.

Authorized Replay uses the same bearer-owning HTTP client consumed from the
one-shot live bootstrap. The Console accepts only a verified Replay for its
exact Room and displayed sequence, and renders hashes and Pack identity rather
than the returned Projection bytes. Dual evidence crosses a separate
application-owned event pair: `worldstream:negotiate-evidence-requested`
carries only Room ID and sequence, while
`worldstream:negotiate-evidence-ready` returns exact proof-package JSON for
that head. The Console validates the v1 wrapper, Room/head, Pack revision and
physical bundle identities, preserves the exact JSON text for download, and
renders counts only. Cryptographic authority remains the separate
`worldstream-negotiate-verify` offline verifier.

`scripts/negotiate-golden-flow.py` is the packaged local acceptance lane. It
runs the exact released `.wspack` twice through fresh production Bundle
Verifier, Component Host, and Core registry processes; checks all nine Actions,
the after-approval restart checkpoint, Outcome, private views, retained lookup,
and evidence cross-index; and runs the independent oracle, public TypeScript
Pack, Console reconnect/privacy, and offline verifier suites. Its receipt sets
`release_evidence=false` and explicitly does not claim the still-separate live
persisted-Room process-restart drill.

# Hosted Activity Platform formation model

This document explains the Hosted Activity Platform through the familiar idea
of a multiplayer game room. The canonical language is in
[CONTEXT.md](../CONTEXT.md). The governing boundaries are
[ADR 0019](adr/0019-separate-hosted-activity-platform-from-worldstream.md),
[ADR 0020](adr/0020-use-vercel-for-control-and-fly-for-direct-browser-streams.md),
and
[ADR 0021](adr/0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md).
The optional Platform Operator-funded agent path is bounded separately by
[ADR 0022](adr/0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md).
The Supabase store and result-reconciliation contract is frozen by
[ADR 0023](adr/0023-use-supabase-for-platform-coordination-and-replay-verified-results.md).
The single-authority hobby-preview packaging, secrets, maintenance, and recovery
contract is frozen by
[ADR 0024](adr/0024-operate-a-single-authority-hobby-preview.md).

## The short explanation

The public product should feel like a normal multiplayer lobby:

1. Choose an activity.
2. Claim a seat and invite other participants.
3. Start when the required roster is complete.
4. Participate or watch in realtime.
5. Keep the result and Replay after the activity ends.

The implementation remains Pack-neutral. A Room can contain a game,
negotiation, social experiment, or another bounded shared activity.

## Familiar game language and canonical terms

| Familiar product idea | WorldStream model |
| --- | --- |
| Choose a game or mode | Select an Activity Listing Revision |
| Waiting room and invitations | Launch Request, Seat Claims, and Seat Invitations |
| Start the match | Freeze the roster and create the Room at Genesis |
| Player slot | Membership with a Pack-defined Role |
| Running match | Room governed by one Activity Pack Revision |
| Match record | Activity Run referencing the authoritative Room |
| Score or result | Pack-defined Outcome and derived Indexed Activity Result |
| Match history | Canonical History and Replay |

Product UI may use familiar phrases such as **set up activity** or **waiting
room**. Technical contracts use **Launch Request** before Genesis and **Room**
after Genesis. This distinction prevents the platform waiting page from
becoming a second source of activity truth. It also avoids confusing the
pre-Genesis waiting page with a Pack-defined Lobby Activity Phase inside an
already-created Room.

## Lifecycle

### 1. Reusable activity definition

An Activity Listing Revision is reviewed and immutable. It fixes the exact
Pack and Activity Client identities, reviewed launch choices, stable seat IDs,
Pack Roles, requiredness, allowed participation kinds, public-viewing policy,
and result-publication policy.

A stable Activity Listing points to its current immutable revision for
discovery. The MVP revisions are canonical checked-in JSON documents identified
by BLAKE3 digest. CI validates the exact referenced Pack and client artifacts,
and Fly independently allowlists each revision digest.

The browser sees reviewed listing copy and bounded choices, not a raw Pack
digest, Role definition, Room Setup Specification, client URL, or arbitrary
configuration surface.

The listing's seat definitions are reusable templates. For example, every new
Agent Heist Run can contain a `navigator` seat.

### 2. Pre-Genesis formation

Starting an activity first creates a Launch Request, not a Room. The creator
may claim an eligible seat or spectate. Each remaining seat can have its own
Seat Invitation. Creation gives no special Room or Host authority.

Supabase durably owns this platform-only formation: Launch Requests, atomic
claims and invitations, quota reservations, and the stable correspondence to
one Host Room Setup Operation. Vercel is the only application backend that
uses that store. Browsers and Fly hold no Supabase database key.

A Seat Invitation is a one-use claim link bound to one Launch Request and one
seat. It uses at least 128 random bits, is stored only by digest, and expires
after 24 hours. After GitHub sign-in, the first eligible atomic claimant wins.
The seat itself is not permanently consumed. Before provisioning, a claim may
be released or reset and a new invitation may be issued. The creator's
coordination powers end when provisioning starts; they do not become Host
Operator authority. One account may claim one seat by default; only a reviewed
solo-test Listing Revision may allow more.

When its Activity Listing Revision permits House Agent fill, the creator
chooses either `humans_only` or `fill_with_house_agents` and sees the fixed,
Platform Operator-funded allowance. That choice freezes after the first claim.
The creator can later request **Start with House Agents**, which gives humans
and their agents one final 30-second claim window before the platform fills
only eligible empty seats.

The MVP pool contains two reviewed, inexpensive House Agent Revisions. It
selects randomly without replacement, permits at most two House Agents in one
Run, records the candidate and selected revisions, and never rerolls or
substitutes an assignment. The draw first freezes the selected revisions and
stable Runner reservation operations. Only after every exact Fly reservation
succeeds does one transaction create the new launch-lineage Agent references
and immutable House Agent Assignments. A terminal reservation failure creates
no partial assignment set. Observed Genesis carries the assignments into the
one Activity Run. WorldStream still receives ordinary Agent Principals and
Memberships.

One per-seat managed House Runner unit serves each assignment on the Fly
Machine, outside `worldstreamd`. Its authority-holding assignment helper and
authority-free model host preserve the existing managed Runner isolation. The
model host uses only the immutable Platform Operator policy, current authorized
Projection, and exact Action Offers. It has no tools, creator prompt, durable
memory, or cross-Run memory. OpenRouter is a replaceable model-host gateway;
its Platform Operator credential never enters Supabase, Rooms, Packs, clients,
or WorldStream.

Each assignment permits at most ten model calls, 120,000 total input tokens,
10,000 total output tokens, 12,000 input and 1,000 output tokens per call, one
in-flight call, and a 60-second timeout. The platform reserves the maximum
exposure before dispatch. An ambiguous call consumes its reservation. There
are no model-call redispatches, provider or model fallbacks, model swaps,
replacement agents, or rerolls. Idempotent WorldStream and Controller
reconciliation may still recover the same exact assignment without another
model call.

Only Listings with bounded response deadlines, exact Action Offers, bounded
Activations, a Pack-defined no-Action consequence, and provider-safe input may
enable fill. Agent Heist qualifies first; the current Negotiate revision does
not. Provider or Runner failure after start produces no invented Action, and
only the Pack decides the consequence and Outcome. Every House-filled Run is
permanently marked **Exhibition — platform-supplied agents**.

### 3. Room creation

When every required seat resolves to an exact server-derived setup Principal
reference through a Seat Claim or House Agent Assignment, the creator's first
start gate freezes the roster, required non-seat spectators, and complete Room
setup intent. It then authorizes Genesis in the Pack-defined Lobby. The platform
maps this one Launch Request to at most one Host Room Setup Operation.

Room Setup Specification v1 cannot express non-seat spectators. The hosted
launcher must first add a bounded versioned spectator field or an equivalent
narrow Host contract. It cannot turn platform spectators into operators or
reuse a participant credential.

Genesis creates the authoritative Room and makes exactly one Activity Run
correspondence recoverable. Vercel records or read-repairs that correspondence
idempotently in Supabase; an outage may delay the row but can never authorize a
second Room. From Genesis onward, WorldStream owns Memberships, ordering,
Projections, Actions, Activity Phase, Outcome, Canonical History, and Replay.

At the second gate, the platform waits for every required client and House
Runner to become synchronized and then automatically submits only the Pack's
declared Lobby launch operation. Neither gate gives the creator a general Room
or Host Operator interface.

Every public Listing Revision also sets a bounded pre-start Room deadline,
initially 30 minutes. Before the Lobby launch commits, the creator may abandon
the activity through one narrow operation. The platform requests the same
operation when the deadline expires. Fly first proves that the Pack's declared
Lobby launch is still applicable and then archives the Room. The Activity Run
retains an abandoned coordination disposition, not an Activity Phase or
Outcome, releases its live capacity, and has no Indexed Activity Result. This
operation cannot archive a Room after the Lobby launch commits.

### 4. Participation and spectating

The Host selects the approved Activity Client for each Membership and issues an
opaque, one-use browser handoff. The browser later obtains a short-lived Stream
Admission Ticket and connects directly to WorldStream on Fly. Vercel remains
the HTTPS product and control plane; it does not relay WebSockets.

Supabase retains one immutable Run Membership Correspondence for every frozen
participant, elected creator spectator, result indexer, and optional public
relay. An owned-Run read returns a random opaque selector for each
account-controlled correspondence. Entry resolves the authenticated account,
Run, and selector on the server, then obtains a fresh one-use Host handoff; the
browser never selects a Principal or Membership identifier.

When the Activity Listing Revision permits anonymous viewing, one
platform-controlled Public Projection Relay republishes only the authorized
Public Projection. Anonymous viewers receive no Membership or participant
authority.

Every hosted Run uses another dedicated result-indexer Spectator Membership.
It is not the Public Projection Relay and cannot act. Fly keeps its credential;
Vercel pulls its authorized public evidence and runs the exact deterministic
Result Projector Revision pinned by the Listing. The launcher provisions this
Membership as part of the frozen setup before Genesis. It is not a Pack seat,
has no Pack Role, and does not change participant cardinality. It lets the
platform determine terminal capacity disposition without Pack-specific shared
code and supports result publication when enabled. Its credential has exactly
attach, public-observation, and Replay scopes; the live relay and creator
spectator have attach and public-observation scopes only. If it cannot be
provisioned, the hosted Listing does not reach Genesis.

The anonymous Activity Client connects directly to a separate read-only Fly
WebSocket endpoint addressed by the random public Run ID. The relay keeps its
Spectator Membership credential server-side. Public browsers receive no
WorldStream credential, and Fly applies exact production-Origin checks,
connection and rate limits, heartbeat enforcement, bounded frames, and
slow-consumer closure.

### 5. Completion and rematch

The pinned projector interprets the authorized Public Projection as
nonterminal, terminal without an Outcome, or terminal with a bounded summary.
A terminal disposition inserts one immutable Run Terminal Evidence record and
releases platform Run capacity in the same database transaction. A public
result appears only when publication is enabled and healthy Room integrity, the
exact Public Projection and Complete Head, and authorized Replay verification
agree. The observed and Replay Projection hashes must match the exact bytes
given to the projector. The Room can later be archived, but its result and
Replay remain separate: the result is a derived public summary and Replay
remains WorldStream evidence. A contradictory terminal disposition or a later
nonterminal reversion quarantines the Run and suppresses or blocks publication;
it never reacquires capacity. The Room is not reused for another round.

A rematch creates a new Launch Request, new Seat Claims, new Room, and new
Activity Run. The same people or agents may claim equivalent seats again.

```text
Agent Heist listing
└── navigator seat template
    ├── Run A → Alice claims navigator
    ├── Run B → Bob claims navigator
    └── Run C → Alice claims navigator again
```

Within Run A, Alice may disconnect and reconnect without reclaiming the seat.
Another Principal cannot replace Alice after Genesis. This preserves private
information, attribution, Outcome integrity, and Replay correctness.

Each hosted Listing pins one reviewed Result Projector Revision. The
projector runs in Vercel, consumes only the result-indexer Membership's public
evidence, and returns either not-terminal, terminal-without-Outcome, or one
bounded schema-valid summary. There is at most one immutable Indexed Activity
Result per publicly indexed Run. The same source and summary reconcile
idempotently; a later Head with the same summary adds verification evidence.
The same source with different output or a later Head with a different summary
is quarantined and never overwrites the first result.

Results show reviewed seat pseudonyms by default. A person may opt into showing
their current GitHub profile, but that mutable profile is joined only when the
page is read. External agents are labelled **External agent — unverified** and
publish no claimed model metadata. House Agents disclose their exact revision
and remain **Exhibition — platform-supplied agents**. Account deletion removes
the login and profile while retained shared history remains pseudonymous and
linkable. A later integrity problem suppresses the public summary without
changing the Room or internal evidence. Visibility folds by the greatest
accepted integrity generation, never arrival time; conflict, privacy, and purge
events cannot be reversed by a later automated recheck.

## The boundary is generic

| Activity | Example fixed seats | Pack-defined result |
| --- | --- | --- |
| Agent Heist | Navigator, Insider, Broker | Heist Outcome |
| Negotiation | Buyer Agent, Seller Agent, Human Approver | Agreement committed or formation expired |
| Social experiment | Moderator, Participants, Observers | Scenario-specific Outcome |

The Hosted Activity Platform owns discovery and pre-Genesis coordination. The
Activity Pack owns the rules. WorldStream owns the authoritative Room. The
Activity Client owns presentation. No game-specific launcher branch belongs in
the kernel.

## Platform launch state

The Hosted Activity Platform exposes only coordination and reconciliation
state before a Run exists:

```text
collecting_roster
        |
        v
provisioning <-> reconciling
        |
        v
run_created

Terminal before Genesis:
cancelled | expired | failed_pre_genesis
```

The same account-scoped idempotency key and canonical launch input return the
same Launch Request. Reusing the key with different input is a conflict. Start
always resumes the same Host Room Setup Operation. An ambiguous response moves
to `reconciling`; it never authorizes a replacement Room. Any observed Genesis
creates exactly one Activity Run, even if client handoff or later provisioning
needs repair. If Fly somehow proves Genesis after a violated platform
authorization boundary, the platform still records that one Room and
quarantines the violation rather than pretending the Room does not exist.

The platform does not add `live`, `turn`, `winner`, or similar states. After
`run_created`, the Activity Client reads Activity Phase and Outcome from
WorldStream.

The public product exposes only fixed catalog-read, Launch Request create and
status, Seat Invitation claim, claim release or reset, start, and enter-Run
operations. Browser input cannot select a Pack, Role, Room Setup Specification,
client URL, Host route, or fallback. Entering a Run resolves the signed-in
account, Run, and opaque entry selector to one immutable Run Membership
Correspondence and obtains a fresh Host-issued client handoff.

## Availability and capacity

The hobby deployment defaults to one nonterminal pre-Genesis Launch Request
and one nonterminal owned Activity Run per Platform Account, with ten
concurrent Runs globally. Collecting, provisioning, and reconciling requests
all consume that launch quota. Joining another account's Run does not consume
the creation quota. These are deployment values rather than new Room rules.

A Launch Request stays pinned to its accepted Activity Listing Revision.
Ordinary delisting blocks new Launch Requests but does not rewrite an accepted
one. A security revocation, missing Pack, or unavailable required Activity
Client blocks Genesis until the dependency recovers or the request reaches its
24-hour expiry. The platform never upgrades the revision, substitutes
Inspector, or falls back to another Pack or client.

Supabase keeps independent `pre_genesis` and `active_run` capacity
reservations. Start acquires Run capacity before any Fly mutation without
dropping the pre-Genesis reservation. That same transaction locks every
controlling account, rejects erasure, and records the first-Host-mutation
boundary. Before the boundary, exact locked platform state may prove a local
failure; after it, failure needs authenticated evidence for the retained Fly
operation. Genesis releases the first reservation; projector-established
terminal disposition releases the second. Ambiguous Host state retains
capacity. One locked global row serializes the ten-Run limit. An
authorization-violating but already-existing Room is recorded and quarantined;
if that temporarily exceeds the limit, new launches stop until capacity falls
within it.

## Accepted MVP platform choices

- Public launch initially accepts only Lobby-compatible Activity Listing
  Revisions. Agent Heist is first. The current active-at-Genesis Negotiate
  revision remains unavailable until it has a separately reviewed safe start
  contract.
- A Launch Request and its Seat Invitations expire after 24 hours.
- Seat Invitations are seat-specific, one-use, GitHub-authenticated, carried in
  a scrubbed URL fragment, and stored only by digest.
- The creator may rotate an unused invitation or reset a claim before
  provisioning. A claimant may release their own pre-Genesis claim.
- One canonical JSON document defines each immutable Activity Listing Revision;
  its identity is a BLAKE3 digest. Vercel and Fly use the same checked-in set,
  while Fly independently enforces its exact allowlist.
- A Platform Account maps to no stable WorldStream Principal. Every human
  seat, creator spectator, and browser-agent claim receives a distinct
  run-scoped Principal without storing a reusable profile, prompt, model
  credential, or verified model identity. Retrying one setup operation retains
  its original Principal identities.
- A Listing Revision may permit House Agent fill. At Launch Request creation,
  the creator selects `humans_only` or `fill_with_house_agents` under the
  Platform Operator's fixed hard per-Agent token allowance. If opted in, **Start with
  House Agents** opens a final 30-second human-claim window. The platform then
  fills only eligible empty seats from the reviewed pool, records every exact
  assignment, and automatically begins after all required clients and Runners
  are ready. These Runs are exhibitions until a later evaluation contract says
  otherwise. ADR 0022 freezes the exact two-revision pool, OpenRouter-backed
  external Runner, allowance, secret, readiness, and failure behavior.
- Public viewing is `disabled` or `anonymous_by_link`. An anonymously viewable
  Run receives one stable random `/runs/{public_id}` URL only after Genesis. It
  reveals no Room or Membership identifier, is not placed in a live directory,
  and remains useful for the final public result after live play ends. A
  result-only Run may receive the same kind of Vercel URL, but its identifier
  is never bound to Fly's anonymous live endpoint.
- The default hobby limits are one pending launch and one nonterminal owned Run
  per account, plus ten concurrent Runs globally. Joining another account's Run
  does not consume its creation quota, and deployment configuration may tune
  the numeric limits.
- A public Listing sets a pre-start Room deadline, initially 30 minutes. The
  creator may abandon before Lobby launch, and the platform archives a still-
  unstarted Room at the deadline. The retained Activity Run is marked abandoned
  without an Indexed Activity Result and no longer consumes live capacity.
- Supabase is authoritative for platform-only formation and correspondence;
  WorldStream remains authoritative for every Room fact. Browsers and Fly have
  no Supabase database key.
- Public result publication is fixed and disclosed by the immutable Listing
  Revision and is independent of catalog visibility. A participant joining a
  result-public Listing cannot opt the Run out after Genesis.
- One digest-pinned deterministic projector runs in Vercel for each supported
  Listing Revision. It uses a dedicated Spectator Membership distinct from the
  optional live-view relay, determines terminal capacity disposition, and adds
  no Pack operation.
- A public result requires a final projected summary, healthy integrity, exact
  Projection and Complete Head evidence, an Outcome, and verified Replay
  through that Head. One immutable result exists per Run; a later Head yielding
  the same summary is confirming evidence, while a divergent summary conflicts
  rather than overwriting it.
- Fly hints are best-effort wake-ups. Vercel pulls facts, read-repairs on status
  and Run views, and performs one bounded daily Hobby sweep.
- Public attribution is pseudonymous by default. Current GitHub presentation is
  opt-in and removable; external agents remain unverified; House Agents expose
  exact immutable exhibition attribution.
- The MVP has Agent Heist Recent Results only. It has no leaderboard, universal
  score, cross-Pack comparison, or verified claim about an external model.

---
status: accepted
date: 2026-09-04
---

# Use reviewed pre-Genesis formation for hosted Activity Runs

## Context

[ADR 0019](0019-separate-hosted-activity-platform-from-worldstream.md)
separates the public Hosted Activity Platform from WorldStream, and
[ADR 0020](0020-use-vercel-for-control-and-fly-for-direct-browser-streams.md)
authorizes its narrow Vercel control plane and direct Fly realtime path. Those
decisions do not yet define how an ordinary person selects an Activity, forms
a roster, starts exactly one Room, or shares a public spectator link.

The Room Setup Specification requires exact Principals, Roles, Pack identity,
and configuration before Genesis. Some Packs also begin meaningful time at
Genesis. Creating a Room before its required participants are known would
therefore require mutable placeholder identities, Pack-specific setup
branches, or timers that run while people are still joining. Allowing the
browser to submit a full Room Setup Specification would expose Host choices and
turn the public product into a remote administration surface.

The public experience should feel familiar to users of multiplayer lobbies,
but neither the Hosted Activity Platform nor WorldStream may become
game-specific. Agent Heist, negotiation, and later social experiments must use
the same formation contract.

## Decision

Use a reviewed **Activity Listing Revision** and an idempotent, pre-Genesis
**Launch Request** to collect one fixed roster before creating a Room. The
product may call this a waiting room, but the canonical Room begins only at
Genesis.

The generic formation is:

```text
Activity Listing Revision
          |
          v
Launch Request -- Seat Claims / Seat Invitations
          |
          v
freeze exact roster and Room Setup Specification
          |
          v
one Room Setup Operation -- Genesis --> one Activity Run
          |
          v
Pack-owned Activity Phases, Outcome, and Replay
```

### Reviewed catalog

One canonical JSON document defines each immutable Activity Listing Revision.
Its identity is the BLAKE3 digest of its canonical bytes. A stable Activity
Listing may point to a current revision for discovery, but every accepted
Launch Request remains pinned to the exact revision it selected.

Each revision fixes all of the following:

- the exact Activity Pack Revision;
- the required Activity Client Release and Client Surface;
- catalog visibility as `public`, `unlisted`, or `private`;
- the small reviewed launch-input schema, defaults, and deterministic
  transformation into one complete Room Setup Specification;
- stable seat IDs, Pack-defined Roles, requiredness, and allowed human,
  browser-agent, or House Agent participation kinds;
- creator access as `must_claim_seat` or `may_spectate`;
- public-viewing policy, initially `disabled` or `anonymous_by_link`;
- the bounded pre-start Room deadline; and
- result publication as `disabled` or `public_recent_results`;
- the exact Result Projector Revision digest, accepted Public Projection
  schema, terminal and Outcome interpretation, bounded public summary schema
  and canonicalizer, and output limit; and
- when publication is enabled, its attribution and public-output policy.

The browser receives reviewed listing copy and a small set of bounded choices.
It cannot submit or select a Pack digest, raw Pack Role, seat definition, Room
Setup Specification, Activity Client URL, Host route, executable artifact, or
arbitrary configuration. A promised client being unavailable makes the
revision unavailable; the platform does not substitute Inspector.

The checked-in canonical documents are the MVP source of truth. CI validates
their canonical identity and every referenced Pack and client artifact. Vercel
and Fly receive the same set, and Fly independently allowlists the exact
revision digest before it accepts setup. A Supabase copy is only a derived read
model of that checked-in Listing document. This does not make all Supabase data
derived: ADR 0023 makes Supabase authoritative for the separate platform-only
formation records described below.

The catalog, launch page, and invitation-claim page disclose the pinned result
publication and attribution policy before a person joins. Joining a Listing
whose policy is `public_recent_results` accepts pseudonymous publication; there
is no post-Genesis per-Run opt-out. Immutable catalog visibility controls only
discovery and cannot enable or disable result publication.

Ordinary delisting blocks new Launch Requests but does not rewrite an existing
request. Security revocation, a missing Pack, or an unavailable required
client blocks Genesis until recovery or the Launch Request's 24-hour expiry.
There is no automatic revision upgrade, Pack fallback, or client fallback.

### Seats, claims, and invitations

A Launch Request contains the Listing Revision's reusable seat templates. A
claim binds one authenticated Platform Account and a server-derived,
setup-local Principal reference to one exact seat before Genesis. Provisioning
allocates a new Principal for that Run. The claimant cannot change the seat's
Role or participation policy. By default an account may claim one seat in a
Launch Request; an explicitly reviewed solo-test Listing Revision may permit
more.

The creator coordinates formation and may claim an eligible seat or, only when
the Listing permits it, elect one creator-spectator Membership. Creation by
itself grants no Activity Role, Membership authority, or Host Operator
authority.

A Seat Invitation is bound to one Launch Request and one exact seat. It uses at
least 128 bits of randomness, is stored only by digest, expires with the Launch
Request after 24 hours, and is carried in a URL fragment that the product
immediately removes from browser history. GitHub sign-in is required before
claim, and the first eligible atomic claimant wins.

Supabase is the authoritative store for Launch Requests, Seat Claims, Seat
Invitations, platform quota reservations, and their immutable Host-operation
correspondence. Claim, release, reset, rotation, roster freeze, and quota
transitions are atomic database operations behind the Vercel BFF. The browser
cannot write these tables or choose its controlling Platform Account.

Before provisioning, a claimant may release their own claim and the creator
may reset a claim or rotate an unused invitation. Provisioning freezes every
claim and the complete Room Setup Specification exactly once. After Genesis,
the same Principal may reconnect to its Membership, but a different Principal
cannot replace it. A seat template remains reusable in later Runs; only its
claim in this Launch Request is consumed.

Genesis promotes every frozen participant and Spectator Membership into one
immutable private Run Membership Correspondence containing its exact purpose,
Access Mode, Principal, and Membership. Participant rows also retain their seat
and Role. Post-Genesis Run entry resolves this correspondence, not the mutable
pre-Genesis Seat Claim. Public responses never expose the correspondence
identifiers.

A Platform Account does not correspond to one stable WorldStream Principal.
Each claimed human seat, external browser agent, and creator spectator receives
a distinct run-scoped Principal, while a retry of the same Room Setup Operation
retains its original identity. The MVP does not store reusable agent profiles,
prompts, model credentials, or verified model-identity claims.

Fixed roster and readiness requirements are Hosted Activity Platform policy.
They do not change kernel or Pack correctness when an optional Participant or
Runner is absent outside this hosted launch path.

### Optional House Agent fill

A Listing Revision may mark particular agent seats eligible for
`house_agent_fill`. The creator must explicitly opt in, accept the Platform
Operator's fixed hard token allowance for each House Agent assignment, and
first allow a bounded period for invited humans or their agents to claim those
seats. The platform may then randomly assign only the remaining eligible seats
from its reviewed Platform Operator-owned pool. The Platform Operator funds
the provider calls; the creator incurs no charge in the MVP.

Every assignment freezes before Genesis and records the exact provider, model,
policy revision, permitted tools, and token allowance. The Run's maximum House
Agent allowance is the sum of those per-assignment allowances. Required House
Runners must be ready before the Pack begins, and no silent model substitution
occurs during a Run. The Runner remains external to WorldStream and owns model
calls and provider credentials; WorldStream sees an ordinary Agent Principal
and Membership. Runs using this fill path are labelled exhibitions rather than
benchmark evidence.

This decision authorizes only that product boundary. The exact pool, Runner,
provider, allowance, secret, readiness, and failure contract is frozen by
[ADR 0022](0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md).
It may not grow into user-stored credentials, reusable profiles, or a general
hosted-agent service by implication.

### Start, idempotency, and reconciliation

Formation has two distinct gates:

1. After every required seat resolves to an exact server-derived Principal
   reference through a Seat Claim or House Agent Assignment, the creator's
   bounded pre-Genesis authorization freezes the roster and Room Setup
   Specification and permits creation of one Room in its Pack-defined Lobby.
   That frozen setup also
   provisions the platform's dedicated result-indexer Spectator Membership,
   the elected creator spectator, and the distinct Public Projection Relay
   when their Listing policies require them. None is a Pack seat or receives a
   Pack Role. Failure to provision a required Membership blocks Genesis rather
   than silently weakening the promised path.
2. After every required Activity Client and House Runner is ready, the platform
   automatically submits only the Listing Revision's exact reviewed Pack
   Lobby-launch operation.

The result indexer's server credential has only attach, public-observation, and
Replay scopes. The Public Projection Relay has attach and public-observation
scopes only. Creator spectators receive attach and public-observation scopes,
but neither participant Action nor Replay authority in the MVP.

Neither gate grants a generic Action, Room, or Host operation. The current
active-at-Genesis Negotiate revision cannot use this path; it must gain a safe
Lobby-compatible revision or a separately reviewed start contract. Agent Heist
is the first hosted Listing even though Negotiate remains WorldStream's first
serious public Pack in the core release narrative.

Room Setup Specification v1 cannot express a non-seat Spectator Membership.
Implementation must first add a versioned bounded spectator field or an
equivalently narrow frozen Host launcher contract. Hosted spectators cannot be
substituted with Operator Memberships or shared participant credentials.

Launch creation uses an account-scoped idempotency key. The same key and
canonical input return the same Launch Request; the same key with different
input conflicts. The frozen Launch Request maps to at most one durable
Host-local Room Setup Operation. Every start retry resumes or queries that
operation. A timeout or ambiguous Fly response enters reconciliation and never
authorizes a replacement operation or Room.

The platform generates and stores the stable Host Room Setup Operation identity
before the first Fly call. Database uniqueness covers account/idempotency key,
Host setup-operation identity, Launch Request-to-Run, and installation/Room.
The original terminal request or idempotency tombstone remains after the
24-hour formation expiry, so a late identical retry does not create another
Room. A changed canonical input always conflicts.

The Hosted Activity Platform exposes only these formation states:

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

Only a terminal failure proven to precede Genesis has no Activity Run. Any
observed Genesis creates or recovers exactly one Activity Run, even if later
client handoff or provisioning repair is required. After `run_created`, the
platform does not duplicate `live`, `turn`, `winner`, or other Pack state;
Activity Phase and Outcome come from WorldStream.

Vercel read-repairs correspondence whenever a creator reads Launch Request
status or an eligible Run page. Fly may send an opaque best-effort wake-up hint,
but Vercel always pulls the facts back from Fly. A bounded once-daily Hobby
sweep covers missing Genesis correspondence, unpublished eligible results, and
published results due for integrity revalidation. A conflicting Room, Run,
roster, Head, or projector identity enters internal reconciliation quarantine
and is never overwritten.

Each public Listing Revision sets a bounded pre-start Room deadline, initially
30 minutes. Before the Lobby launch commits, the creator may request
`Abandon activity`; the platform requests the same narrow operation when the
deadline expires. Fly must first prove that the declared Lobby launch remains
applicable and may then archive the still-unstarted Room. The platform records
the verified archive as an abandoned coordination disposition, not an Activity
Phase or Outcome. The Activity Run is retained, releases its live capacity,
and has no Indexed Activity Result. This path is unavailable after the Lobby
launch commits. A rematch always creates a new Launch Request and Room.

### Public launcher and entry surface

The platform API is fixed and bounded to catalog reads, Launch Request
creation and status, Seat Invitation claim, claim release or reset, start, and
enter-Run. Entering a Run resolves the signed-in account, Run, and BFF-issued
opaque entry selector to one immutable Run Membership Correspondence and asks
the Host for a fresh one-use Activity Client handoff. The selector grants
nothing without the matching account and can disambiguate multiple seats under
the reviewed solo-test exception. No route accepts a Principal, Membership,
generic Host operation, or caller-selected upstream.

Vercel handles these HTTPS controls. Participant Activity Clients obtain a
short-lived Stream Admission Ticket from their Browser Activity Session and
connect directly to Fly as defined by ADR 0020.

A Run whose Listing permits anonymous live viewing or public result publication
receives one stable identifier generated from at least 128 random bits only
after Genesis, in the same Supabase transaction that creates its Run
correspondence. Vercel binds it to the exact Fly Room only when the Listing
permits `anonymous_by_link`; a result-only identifier cannot resolve at Fly's
live endpoint. The URL is not advertised as live until a required binding is
confirmed. It is available at `/runs/{public_id}`. There is no public live Run
directory in the MVP. The same URL shows a Replay-verified final public result
when eligible.

Agent Heist may also appear in a bounded chronological Recent Results view.
This is not a leaderboard. A public Run or result DTO reveals no Room,
Membership, Principal, Platform Account, setup-operation, invitation, or
Supabase row identifier. A suppressed result page returns only a generic
unavailable state and no prior summary or participant attribution.

Public results use reviewed pseudonymous seat labels by default. A participant
may separately enable their current GitHub login and avatar for presentation;
that profile is joined at read time and is not copied into immutable result
evidence. Profile disable or account deletion falls back to the seat pseudonym
and `Deleted participant` without deleting shared history. External agents are
labelled `External agent -- unverified`; client-claimed model, provider, prompt,
tool, skill, and agent names are ignored. House Agent attribution remains exact
and permanently marked as an exhibition under ADR 0022.

Anonymous live viewing is an isolated exception to participant admission. It
uses a separate read-only Fly WebSocket endpoint addressed by that public Run
identifier, not the participant endpoint or a `wst1` ticket. A
platform-controlled relay retains one scoped Spectator Membership server-side
and republishes only its Public Projection; the browser receives no
WorldStream credential. Fly enforces the exact Vercel Origin, bounded frames
and connections, rate limits, heartbeat, and slow-consumer closure. Anonymous
viewers can neither act nor request private Replay.

### Hobby deployment limits

The initial deployment defaults to one nonterminal pre-Genesis Launch Request
and one nonterminal owned Activity Run per Platform Account, plus ten
concurrent Runs globally. A request in `collecting_roster`, `provisioning`, or
`reconciling` consumes the pre-Genesis quota. Joining another account's Run
does not consume the joiner's creation quota. These numbers are deployment
configuration, not Room or Pack rules.

Pre-Genesis and active-Run capacity are independent atomic Supabase
reservations. Launch creation acquires the first; start acquires the second
before any Fly mutation while retaining the first; Genesis releases only the
first. Verified projector terminal disposition, proven pre-Genesis failure, or
pre-start abandonment releases the applicable reservation. All global
acquisition and release uses one fixed database lock order. Ambiguous or stale
reconciliation retains capacity and may delay a new launch rather than exceed
the configured limit.

## Consequences

- Users get a familiar choose, invite, start, participate, and watch flow while
  the kernel remains Pack-neutral.
- Required identities and Roles are complete before Genesis, so no placeholder
  Principal, late Role swap, or pre-attendance Pack timer is needed.
- The Hosted Activity Platform coordinates formation but never becomes a
  second authority for Activity state or Outcome.
- Catalog review, Host allowlisting, and artifact approval remain separate;
  an entry in the public catalog grants no execution authority.
- Agent Heist can prove the first public path. Negotiate must change or receive
  a separate reviewed start decision before using the same launcher.
- House Agent fill is a deliberate, narrow expansion whose security and
  execution contract is frozen by ADR 0022.
- The current repository has exact Pack lookup, bounded Room Setup
  Specifications, retained idempotent Room Setup Operations, Memberships, and
  client handoffs. It does not yet implement Listing Revision documents,
  hosted Launch Requests, Seat Invitations, the platform correspondence store,
  Supabase migrations and grants, Run Membership Correspondences,
  result-indexer admission, the projector registry, result evidence,
  reconciliation sweep, or the public projection endpoint.

This decision extends rather than supersedes
[ADR 0019](0019-separate-hosted-activity-platform-from-worldstream.md) and
[ADR 0020](0020-use-vercel-for-control-and-fly-for-direct-browser-streams.md),
with its House Agent exception bounded by
[ADR 0022](0022-use-bounded-openrouter-house-runners-for-exhibition-fill.md)
and its platform-store and result contract frozen by
[ADR 0023](0023-use-supabase-for-platform-coordination-and-replay-verified-results.md).
The single-authority preview deployment and maintenance boundary are frozen by
[ADR 0024](0024-operate-a-single-authority-hobby-preview.md).

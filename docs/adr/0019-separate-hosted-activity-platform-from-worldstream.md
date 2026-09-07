---
status: accepted
date: 2026-09-04
---

# Separate the hosted Activity Platform from WorldStream

## Context

[ADR 0001](0001-product-boundary.md) freezes WorldStream as a self-hosted
realtime Room Runtime and excludes turning it into a game or social
destination. Later decisions made Activity Packs headless, Activity Clients
independent, and Host administration CLI-first. None of those decisions defines
a public application that lets people discover, launch, join, or watch selected
Activities.

The deployment MVP needs such an application for Agent Heist and later
Negotiate. It also needs platform authentication, a reviewed catalog, bounded
Room launch, anonymous spectating, and a public result index. Putting those
concerns into the Room Kernel, Activity Pack, Activity Client, or Host Operator
interface would create a second source of Room truth and reverse ADR 0001 by
accident.

This repository still uses one domain context. The MVP therefore adds a small
set of sharply bounded integration terms to the root glossary rather than
creating an informal second glossary.

## Decision

Build the public experience as a separately branded **Hosted Activity
Platform**, described as powered by WorldStream. WorldStream remains the
generic Room Runtime and keeps its current identity. The Hosted Activity
Platform is an external application, not a hosted mode of the kernel. Its
operating organization may separately act as Application Integrator and Host
Operator for its deployment without merging those authorities.

The ownership boundary is:

| Owner | Owned responsibilities |
| --- | --- |
| Activity Pack | Roles, Actions, visibility, Activity Phases, legality, and Outcome |
| Room and Runtime | Authoritative Room State, Membership, ordering, Canonical History, Replay, and the operational Room Integrity State |
| Host Operator | Pack approval, Room setup authority, Client Deployments, Client Bindings, and Client Selection |
| Activity Client | Pack-aware presentation and interaction through its scoped client contract |
| Hosted Activity Platform | Listings, Platform Accounts, admission policy, bounded launch requests, brokerage of Host-issued opaque handoffs, anonymous republication policy, and derived public indexes |

A Platform Account is not a WorldStream Principal, Membership, or Host
Operator. During launch preparation or later joining, the platform may map an
account-controlled human or agent to a Principal as a correspondence record
only. The Host-created room-local Membership and its scoped credential grant
Room authority. Owning or configuring an agent does not collapse the human and
agent into one Principal.

### Listings and launches

The reviewed, version-controlled public catalog contains stable **Activity
Listings** with immutable, content-identified **Activity Listing Revisions**.
Each revision pins the exact Activity Pack Revision, required Activity Client
Release and Client Surface, allowed launch-input schema and defaults,
participation and public-viewing policy, one exact deterministic Result
Projector Revision, and a versioned result-publication policy. The projector
pins its accepted Public Projection schema, terminal and Outcome
interpretation, output schema, canonicalizer, and size bound because the
platform uses its terminal disposition for capacity reconciliation even when
publication is disabled. When publication is enabled, the policy additionally
pins its attribution and public-output rules. The Listing Revision and bounded
launch inputs are pinned when the Launch Request is created. After the request
gathers its permitted seat claims,
provisioning freezes the complete roster and resolves one complete Room Setup
Specification exactly once. A revision may cite an exact Activity Distribution
as provenance. A Distribution reference is optional for the MVP because the
embedded Agent Heist Pack does not yet have one.

Listing review does not approve artifacts or grant Host authority. The Host
independently allowlists the exact immutable Activity Listing Revision identity
and must still resolve approved Pack and client records, including a compatible
Client Binding. The checked-in immutable revisions are the MVP catalog source
of truth. Any Supabase listing rows are deployed read-model copies and cannot
silently change launch intent.

A **Launch Request** is the idempotent pre-Room request for one exact Activity
Listing Revision. It accepts only schema-allowed inputs, gathers claims for the
revision's fixed seats, freezes one roster and Room Setup Specification when
provisioning starts, and maps to at most one Host-local Room Setup Operation.
Retry or double-click must not create another Room. Only a terminal failure
proven to precede Genesis creates no Activity Run. An unknown or resumable
result reconciles the same Room Setup Operation; observed Genesis creates
exactly one Run even if later provisioning or handoff fails.

An **Activity Run** begins only after Room Genesis and permanently references
exactly one Room and its originating Activity Listing Revision. It is a
platform discovery and coordination record, not a parallel activity state
machine. Room Status, Activity Phase, Membership, Actions, legality, state,
and Outcome are always read from WorldStream. A rematch creates a new Launch
Request, Room, and Activity Run. Replay never creates a Run.

The platform service will operate through a narrow host-side launcher boundary.
That boundary may accept only exact immutable Activity Listing Revision
identities independently allowlisted by Host policy, schema-allowed inputs,
quotas, and idempotency keys. It may broker only Host-issued opaque one-use
client handoffs. A browser or stateless web frontend never receives the Host
Operator credential. This delegation is not a generic remote administration
API.

This ADR fixes ownership but does not itself authorize that remote call or
handoff. [ADR 0020](0020-use-vercel-for-control-and-fly-for-direct-browser-streams.md)
now supplies the separate security decision for the exact Hosted Activity
Platform path. Generic remote Host calls remain blocked.

Starting and joining require platform authentication in the MVP. Joining uses
an opaque seat invitation; the server verifies the invitation and binds its
claimant only to the exact predeclared seat and Role. Users cannot supply
arbitrary member identities or Roles. Open matchmaking is deferred. The exact
reviewed catalog, pre-Genesis roster, invitation, idempotency, start, and
public-link contract is frozen in
[ADR 0021](0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md).

### Clients and public viewing

An Activity Listing that promises a specialized Activity Client is unavailable
when that approved client cannot be selected. The public platform does not
silently substitute WorldStream Inspector. Inspector remains available for
authorized Memberships under the generic Host fallback decision in
[ADR 0017](0017-separate-activity-clients-from-packs-and-studio.md); the public
platform simply does not expose that fallback as the promised polished
experience.

Anonymous live viewing is disabled by default and may be enabled explicitly
for an Activity Listing. When enabled, a platform-controlled **Public
Projection Relay** connects through a restricted Spectator Membership and
republishes only the authorized Public Projection. Anonymous visitors receive
neither a WorldStream credential nor participant authority. This avoids
unbounded durable Membership creation while preserving the rule that
`Public` does not mean unauthenticated kernel access.

WebMCP tools remain part of each exact Activity Client Release or a direct
protocol client. The platform shell does not interpret Pack Actions, duplicate
Action Offers, or contain Heist-, Negotiate-, or game-specific behavior.

### Public results

An **Indexed Activity Result** is produced under the exact Activity Listing
Revision's versioned result-publication policy only after all of these
conditions hold:

- the pinned Result Projector Revision yields a final schema-valid summary;
- Room integrity is healthy;
- the summary uses only a Public Projection produced through the existing
  Activity Pack view contract; and
- authorized Replay verifies through the same exact Complete Head and public
  Projection evidence; and
- the record retains exact Room, Activity Pack Revision, and canonical-head
  provenance.

There is at most one immutable Indexed Activity Result per Activity Run. An
exact rebuild under the Run's pinned policy is idempotent, and a later Head
yielding the same summary adds verification evidence without replacing the
result. The same source yielding different output or a later Head yielding a
different summary is quarantined rather than overwritten. A later unhealthy or
inconclusive integrity observation suppresses the Published Activity Result
while retaining the immutable internal evidence. A projector change creates a
new Listing Revision for future Runs and never recomputes an earlier Run.

The index may lag or be rebuilt without changing Room truth. Clients and Packs
cannot write it directly. A Room with no Outcome has no result rather than a
score of zero. The platform does not infer a universal score or compare
unrelated Activities.

This ADR adds neither a sixth Activity Pack operation nor a client-authored
Outcome. The exact Supabase, projector, attribution, and reconciliation
contract is frozen by
[ADR 0023](0023-use-supabase-for-platform-coordination-and-replay-verified-results.md).

Delisting prevents new Launch Requests but does not change existing Rooms or
Runs. A changed listing creates new versioned launch intent and never migrates
an existing Run. A client release change cannot silently upgrade a listing or
Run.

## Consequences

- WorldStream remains usable outside the hosted product and gains no
  game/social, account, matchmaking, leaderboard, or marketplace concepts.
- The Hosted Activity Platform may offer a coherent public product without
  becoming a second authority for Room state or outcomes.
- Remote launch and browser handoff require a deliberately narrow security
  boundary; existing local operator authentication cannot be forwarded to the
  web tier.
- Supabase Auth and the platform database are authoritative for Platform
  Accounts and platform-only pre-Genesis coordination, including Launch
  Requests, Seat Claims, Seat Invitations, quota reservations, and immutable
  Host-operation correspondence. Supabase also retains Activity Run
  correspondence, result evidence, and public result records. It never owns
  the MVP catalog source of truth, Room, Membership, Activity Phase, Outcome,
  Replay, authoritative Room state, or private Projections.
- The public catalog is reviewed and version-controlled for the MVP. Creator
  self-service, moderation, ratings, and marketplace behavior remain outside
  this decision.
- If platform accounts, matchmaking, rankings, moderation, or community
  publishing develop into a substantial independent model, introduce an
  explicit context map or move the hosted product to a separate repository.

This decision extends rather than supersedes
[ADR 0001](0001-product-boundary.md),
[ADR 0017](0017-separate-activity-clients-from-packs-and-studio.md), and
[ADR 0018](0018-cli-first-operator-surface.md). It is specialized by
[ADR 0020](0020-use-vercel-for-control-and-fly-for-direct-browser-streams.md)
and
[ADR 0021](0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md),
with its platform-data and result contract frozen by
[ADR 0023](0023-use-supabase-for-platform-coordination-and-replay-verified-results.md)
and its hobby-preview operating contract frozen by
[ADR 0024](0024-operate-a-single-authority-hobby-preview.md).

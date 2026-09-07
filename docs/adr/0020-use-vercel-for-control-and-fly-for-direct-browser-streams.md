---
status: accepted
date: 2026-09-04
---

# Use Vercel for hosted control and Fly for direct browser streams

## Context

[ADR 0017](0017-separate-activity-clients-from-packs-and-studio.md) keeps
Activity Clients independent and requires a separate security decision before
remote HTTPS handoff. [ADR 0019](0019-separate-hosted-activity-platform-from-worldstream.md)
keeps the Hosted Activity Platform outside WorldStream and deliberately leaves
that remote boundary unauthorized.

The public MVP must demonstrate WorldStream's realtime protocol rather than
turning it into a one-second polling service. Vercel can serve WebSockets, but
its implementation is a time-bounded Function beta: every connection is pinned
to one Function invocation, closes at that invocation's maximum duration, and
may reach a different instance after reconnect. Relaying every WorldStream
frame through Vercel would add a second socket, another failure and
backpressure boundary, and transfer cost while leaving Fly responsible for all
Room authority.

Three alternatives were considered:

1. use only the existing cookie-backed HTTP observation and action routes;
2. terminate a browser WebSocket on Vercel and relay it to Fly; or
3. keep hosted control on Vercel while connecting the browser directly to a
   narrowly admitted WorldStream WebSocket on Fly.

The first option hides the kernel's central capability. The second creates a
new realtime intermediary without moving authority. The third requires one new
session-to-ticket seam but keeps the live connection at its actual authority.

## Decision

The Hosted Activity Platform uses a split plane for browser Activity Clients:

| Plane | MVP owner | Responsibility |
| --- | --- | --- |
| HTTPS product, control, and projection | Vercel | Activity Client assets, GitHub sign-in through Supabase Auth, authenticated formation, catalog and result DTOs, Fly evidence pulls, deterministic result projection, handoff and ticket requests, and bounded read repair |
| Platform data | Supabase | Platform Accounts, authoritative platform-only pre-Genesis coordination, immutable Activity Run correspondence and result evidence, and public-result visibility |
| Room and realtime data | WorldStream on Fly | Genesis, direct browser WebSockets, exact Membership admission, Projection Reset, Observation delivery and acknowledgement, Actions and receipts, heartbeat, Catch-up, integrity, and Replay authority |

Vercel exposes no WebSocket endpoint in the MVP. All browser, SDK, Runner, and
service WebSockets terminate on the Fly-hosted WorldStream deployment. A later
Vercel relay requires a separate decision and cannot become an implicit
fallback.

Vercel remains a fixed-route backend-for-frontend, not a generic proxy. Fly
remains authoritative for Principal, Membership, Client Selection, Room,
Projection, Action legality, ordering, Cursor, and Replay. Supabase owns
Platform Account authentication and authoritative platform-only coordination,
but no Room, Membership, Activity Phase, Outcome, Projection, or Replay truth.
The exact platform-store boundary is frozen by
[ADR 0023](0023-use-supabase-for-platform-coordination-and-replay-verified-results.md).

Browser code accesses application data only through the Vercel BFF. It receives
no Supabase database key. Fly also receives no Supabase database key and never
writes the platform database. Fly may send Vercel an authenticated,
best-effort opaque reconciliation hint after Genesis, Room progress that could
change projector disposition, or a later integrity change. The hint contains
no Projection, Outcome, Membership credential, result, or Supabase authority;
Vercel treats it only as a wake-up and pulls authoritative evidence back from
Fly.

Each hosted Run gives a platform-controlled result-indexer Principal a
dedicated, Run-scoped Spectator Membership. That Membership is distinct from
the optional Public Projection Relay and has no participant Action authority.
Fly retains its credential; Supabase retains only its private correspondence.
The reviewed projector itself runs in Vercel and receives only the authorized
public evidence defined by ADR 0023.

### Hosted browser admission

The normal participant flow is:

1. The user completes GitHub OAuth through a server-side Supabase callback on
   the canonical production Vercel origin. Supabase access and refresh tokens
   remain in a host-only, `Secure`, `HttpOnly` platform cookie and are never
   returned to browser JavaScript. ADR 0023 fixes the one-use OAuth-attempt
   store, PKCE/state validation, and exact `__Host-` cookie attributes.
2. A same-origin Launch or Join Request maps the Platform Account and opaque
   entry choice to one newly allocated run-scoped Principal or an existing Run
   Membership Correspondence. The narrow Fly launcher creates or selects the
   exact Membership and performs Host-owned Client Selection.
3. The Host constructs one registered same-origin Activity Client path and
   appends one origin-, Membership-, Client Binding-, and Client
   Deployment-bound `wsh1` handoff in the URL fragment. The handoff is
   single-use, defaults to a 60-second lifetime, and may never exceed five
   minutes. The Activity Client removes it from browser history before loading
   third-party resources or recording analytics.
4. The client sends `wsh1` only in
   `X-WorldStream-Participant-Handoff` to the fixed same-origin redemption
   route. Fly atomically consumes it before responding. A lost response is not
   retried with the same handoff.
5. Successful redemption creates one opaque Browser Activity Session and sets
   exactly this browser cookie:

   ```text
   ws_participant_session=wss1:...; Path=/api/v1/participant-console; Max-Age=43200; Secure; HttpOnly; SameSite=Strict
   ```

   The cookie has no `Domain`, is absolute rather than sliding, and is cleared
   with the same attributes and `Max-Age=0`. It contains no Supabase token,
   Membership bearer, Room ID, Member ID, or Client Binding selection.
6. The client calls the same-origin
   `POST /api/v1/participant-console/session:stream-ticket` route. Vercel
   reconstructs only the `ws_participant_session` cookie on the Fly hop. Fly
   revalidates the current Membership, Membership Standing, Access Mode, Role,
   selected Client Binding, and Client Deployment before using the sealed
   server-side Membership bearer to issue a Stream Admission Ticket.
7. Vercel returns only an opaque `wst1` ticket and its lifetime. The browser
   opens the one compiled `wss://` Fly endpoint with the exact WorldStream
   subprotocol and sends `wst1` as its first text frame. Fly atomically consumes
   the ticket and binds the connection to the ticket's exact Membership and
   Client Selection. Browser-provided identifiers never select a different
   target.
8. After admission, the ordinary WorldStream protocol carries Projection
   Reset, Observations, acknowledgements, Actions, receipts, heartbeat, and
   Catch-up directly between the Activity Client and Fly. First-party clients
   do not poll during normal live operation.

A Stream Admission Ticket is valid for at most 15 seconds, usable once, bound
to the exact production Activity Client origin and browser session target, and
retained server-side only by digest. It never appears in a URL, cookie,
subprotocol, log, error body, or close reason. Its brief presence in browser
JavaScript until the first frame is intentional; no reusable Supabase token,
Browser Activity Session secret, or Membership bearer is available there.

### Vercel gateway contract

The public gateway may expose only these exact participant operations:

- `POST /api/v1/participant-console/handoffs:redeem`
- `GET /api/v1/participant-console/session`
- `POST /api/v1/participant-console/session:observe`
- `POST /api/v1/participant-console/session:acknowledge`
- `POST /api/v1/participant-console/session:act`
- `POST /api/v1/participant-console/session:replay`
- `POST /api/v1/participant-console/session:stream-ticket`

The observation, acknowledgement, action, and Replay HTTP operations remain
bounded compatibility and recovery surfaces. They are not the first-party
client's normal live transport. Operator handoff issuance, Room setup,
Client Binding administration, Runtime administration, arbitrary paths, and a
caller-selected upstream are never reachable through this gateway.

Every state-changing request requires all of the following before request-body
parsing or a Fly call:

- an `Origin` exactly equal to the configured canonical production HTTPS
  origin;
- `Sec-Fetch-Site` either absent or `same-origin`;
- the route's exact method and JSON media type; and
- a server-issued CSRF value bound to the platform session and returned through
  a dedicated request header.

Preview origins, wildcard subdomains, missing or `null` mutation origins, form
posts, and caller-provided forwarding headers are rejected. Read-only requests
accept only an absent or exact Origin and absent or same-origin Fetch Metadata.
The Vercel response emits no CORS permission because the browser HTTP path is
same-origin.

These checks apply equally to every ADR 0021 formation mutation, including
Launch Request creation, invitation claim, claim release or reset, start,
abandonment, and Run entry. They are not limited to participant-console routes.

The gateway uses one configured Fly origin and one least-privilege gateway
credential. It strips browser `Authorization`, Supabase cookies, unapproved
cookies, `Host`, forwarding headers, upgrade headers, and hop-by-hop headers.
It reconstructs only route-specific headers and never forwards a browser
cookie jar wholesale. Responses expose only the allowed status, JSON media
type, the exact participant session `Set-Cookie`, and the delivery
acknowledgement header. Every response uses
`Cache-Control: private, no-store, max-age=0`; external rewrites and caches are
not the security boundary.

Upstream redirects are disabled and every Fly `3xx` is rejected. Launch and
join may return only a server-constructed `303` to a registered same-origin
Activity Client path. There is no caller-supplied `returnTo`, URL, origin,
host, path, or query destination.

### Direct Fly stream contract

The browser stream URL and path are immutable deployment configuration and are
allowed by an exact Content Security Policy `connect-src`. A Fly URL is public
routing information, not a secret. The browser sends no Vercel or Supabase
cookie to Fly.

Fly admits a participant browser WebSocket only when all of these checks pass:

- the request targets the dedicated browser stream path;
- the `Origin` exactly matches the ticket's canonical production origin;
- the requested subprotocol exactly matches the supported WorldStream
  protocol;
- the first message is one bounded text `wst1` frame received before expiry;
- atomic ticket consumption succeeds; and
- global, peer, Principal, Membership, and connection limits admit the stream.

Wrong Origin, preview Origin, missing or wrong subprotocol, binary first frame,
expired ticket, replayed ticket, oversized frame, and capacity rejection close
with generic safe reasons. The public ingress applies pre-upgrade rate limits,
bounded message sizes, heartbeat and idle enforcement, slow-consumer
backpressure, and connection caps. It exposes no operator or generic Controller
surface. Non-browser clients retain their separate header-authenticated SDK
path and do not weaken this browser admission path.

An Activity Listing may separately permit anonymous public viewing under
[ADR 0021](0021-use-reviewed-pre-genesis-formation-for-hosted-activity-runs.md).
That path uses an isolated, read-only Fly endpoint addressed by a random public
Run identifier and a platform-controlled relay that retains its one scoped
Spectator Membership server-side. It never admits the anonymous browser to the
participant endpoint and does not relax the participant `wst1` checks above.

The MVP runs one authoritative Fly deployment with one Runtime and its existing
SQLite authority. Its loopback Controller, narrow Hosted Gateway, and bounded
Runner processes do not create additional Room authorities. It does not
horizontally load-balance one Room or its in-memory Browser Activity Sessions
across Machines. Scaling later partitions Room authority or introduces an
explicit session store; it does not copy authority into Vercel. The exact
single-Machine process and recovery contract is frozen by
[ADR 0024](0024-operate-a-single-authority-hobby-preview.md).

### Continuity and failure

Vercel holds no live-stream or Room state. A Vercel cold start, deployment, or
restart does not interrupt an already open direct Fly stream. Supabase
unavailability blocks new platform sign-ins and authoritative platform
mutations, including launch/join admission, but does not revoke an already
admitted, unexpired Browser Activity Session and cannot stop an existing Room.
Missing Run correspondence, result publication, and integrity rechecks resume
through Launch/Run read repair, authenticated Fly hints, and the bounded daily
sweep in ADR 0023. Lost, duplicated, delayed, or reordered hints have no
correctness effect.

After an ordinary network disconnect, the client uses its HttpOnly Browser
Activity Session to request a new `wst1`, reconnects directly to Fly, and
resumes from its last acknowledged Cursor. It disables Actions until a
Projection Reset or Catch-up establishes an authorized synchronized baseline.
It never retries a consumed handoff or Stream Admission Ticket.

A Fly Controller/Runtime restart closes streams and invalidates in-memory
handoffs, Browser Activity Sessions, and Stream Admission Tickets. The durable
Room, Membership, Cursor, Canonical History, and Replay survive. The next
session operation clears the stale cookie, and the authenticated user must
explicitly rejoin and redeem a new handoff to the same eligible seat.

The MVP supports one reconnectable Browser Activity Session per browser
profile on the production origin. Redeeming a second handoff retires the first
session and its active stream; the earlier tab must rejoin. Run-scoped parallel
browser sessions are deferred.

## Consequences

- The public MVP demonstrates actual WorldStream push delivery and bidirectional
  Actions rather than using polling as its presentation layer.
- The Rust runtime remains the sole realtime authority and data-plane server.
- Vercel remains valuable for static delivery, authentication, public product
  APIs, and platform data without becoming a stateful stream broker.
- The existing `wsh1` to `wss1` path does not yet compose with the Runtime's
  bearer-to-`wst1` path. Implementation must add the narrow
  Browser-Activity-Session-to-Stream-Admission-Ticket operation on Fly; copying
  a Membership bearer to Vercel is forbidden.
- The Runtime's browser Origin policy, Activity Client deployment policy, and
  TypeScript browser transport must gain an exact remote-production mode while
  retaining their loopback development mode.
- A deployed proof must show two browsers receiving pushed state with no normal
  polling, action and receipt delivery, exact-Origin rejection, ticket expiry
  and replay rejection, credential absence in storage and network inspection,
  reconnect with Cursor catch-up, uninterrupted operation beyond five minutes,
  and explicit rejoin with Room recovery after a Fly restart.

This decision authorizes the remote browser contract left open by ADR 0017 and
ADR 0019. It does not authorize generic remote Host administration, anonymous
participant access, a Vercel WebSocket relay, or multi-Machine Room authority.
Its Supabase and result-reconciliation boundary is specialized by
[ADR 0023](0023-use-supabase-for-platform-coordination-and-replay-verified-results.md),
and its preview deployment is operated under
[ADR 0024](0024-operate-a-single-authority-hobby-preview.md).

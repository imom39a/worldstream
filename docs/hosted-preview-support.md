# Hosted hobby preview support and acceptance

This document defines what the first hosted WorldStream preview supports. It
also defines the evidence required before the preview is called ready.

For observed deployment results and outstanding checks, see the
[2026-09-07 deployment record](hosted-preview-status-2026-09-07.md).

## Deployment addresses

The operator has provisioned these MVP endpoints:

| Service | Address | Purpose |
| --- | --- | --- |
| Activity site | <https://worldstream-demos.vercel.app> | Catalog, sign-in, clients, and platform HTTP API |
| Fly authority | <https://worldstream-preview.fly.dev> | Health checks, protected gateway, and direct browser WebSockets |
| Supabase | `grbuawkxcyyubzbzoeux` | WorldStream Free-plan project for Auth and platform data |

These addresses are not an acceptance result. The operator must check the
deployed source identities and complete the tests below before inviting users.
A maintenance response or an older demo gallery does not mean that live match
formation is available. Do not put service keys in links, browser settings, or
support reports.

## Supported product path

The preview has one public activity catalog. Agent Heist is the first activity.
Negotiation and other Activity Packs can use the same platform boundaries in a
later release.

One Agent Heist Run can contain:

- a person signed in with GitHub;
- an invited external browser agent, labelled **External agent — unverified**;
- a reviewed platform House Agent that uses OpenRouter;
- an anonymous spectator; and
- a public, Replay-verified terminal result.

The browser receives control-plane HTTP from Vercel and realtime WebSockets
directly from Fly. Vercel does not provide a WorldStream WebSocket route.
Supabase stores accounts, formation state, Run publication state, and public
results. The Fly volume stores the authoritative Room history and House Runner
state.

Room creation and entry readiness are separate. If setup loses a capability
reply after Genesis, the platform retries the same frozen setup. It does not
offer Run entry until the Host confirms that Room setup is complete. This retry
does not draw another House Agent or create another Room.

## Client acceptance matrix

These are the intended public-preview paths. They are not certified until the
deployed acceptance evidence below passes on each named client:

| Client | Supported use |
| --- | --- |
| Current Chrome desktop | Sign in, form a Run, participate, and spectate |
| Current iOS Safari | Participate and spectate |
| ChatGPT desktop with WebMCP | Read, wait, and submit the reviewed Heist commitment action |
| TypeScript browser client | Direct hosted participant or spectator session |

WebMCP is assisted participation. It is not a claim that an agent can complete
all of Agent Heist autonomously. External agents are not verified by the
platform. Runs that include a House Agent are exhibitions and are not ranked.

## Preview limits

- One Fly authority and one persistent Fly volume.
- No SLA and no automatic multi-region failover.
- At most four concurrent House invocations in the reviewed Runner Template.
- House spending is limited to USD 2 per day and USD 10 per month by the
  retained allowance ledger.
- Recent Results is chronological and limited to 20 Agent Heist results. It is
  not a leaderboard.
- A planned maintenance window can reject new Runs and disconnect active
  clients. Clients must reconnect or re-enter after service returns.
- A normal process restart must preserve acknowledged Room history. An
  acknowledged-history loss blocks release.

A retained House Agent restarts with the same Assignment, reviewed revision,
working directory, and allowance ledger. Restart does not reserve a new House
Agent or refill its budget. If its retained binding, executable, or credential
cannot be verified, restoration stays incomplete instead of replacing it.

### Hosting costs are separate from model limits

Keep the preview on Vercel Hobby and a Supabase Free organization. Do not
enable paid plans, paid add-ons, automatic credit purchases, or extra Fly
Machines without explicit operator approval. Free-plan limits can make the
site unavailable; they are not permission to upgrade the account.

Fly bills resource usage automatically when a payment method is attached.
It does not provide a hard spending cap or billing alerts. Use one shared-CPU
Machine with 512 MB of memory and one 1 GB volume for the initial preview.
Deploy with `--ha=false` so the deployment does not create another Machine.
Do not configure a metrics-based autoscaler, a dedicated paid IPv4 address,
or automatic volume expansion. These resource choices limit the normal
baseline, not the total bill: outbound traffic can still add cost.

When the preview is not in use, an operator can close launches, drain work,
enter maintenance, and stop the Machine. Keep automatic start disabled so
an incoming request cannot start it again. This is planned downtime, not
automatic idle stopping of live Rooms. The volume and stopped Machine root
filesystem still incur storage charges. Do not delete recovery data to save
this small storage cost.

OpenRouter Auto Top-Up must stay off. Use a dedicated expiring key with a
USD 2 lifetime limit for initial acceptance, in addition to the retained
per-assignment token ledger and daily/monthly limits. Never use an unlimited
personal key. An exhausted or expired key makes House Agents unavailable;
the platform must not buy credit or switch credentials automatically.

House Agents also need [operator approvals for the actual installed
records and executable](hosted-house-approval.md). Registering an approval
does not enable it. Review and activate the exact records only after the
deployment, provider privacy, credential, and budget checks pass.

These provider policies were checked on 2026-09-07. Check them again before
changing the deployment:

- [Vercel Hobby limits](https://vercel.com/docs/plans/hobby)
- [Supabase Free-plan cost control](https://supabase.com/docs/guides/platform/cost-control)
- [Fly cost management](https://fly.io/docs/about/cost-management/)
- [Fly storage and usage billing](https://fly.io/docs/about/billing/)

## Not supported

- Generic operator or Studio access over the public internet.
- Arbitrary Activity Pack upload or arbitrary executable execution.
- Public access to private Projections, prompts, provider replies, credentials,
  Room IDs, Membership IDs, or internal authority.
- Ranked model claims, prizes, crypto payments, or a marketplace.
- Autonomous WebMCP play beyond the reviewed tools.
- Vercel WebSockets or a polling substitute for realtime play.

## Local acceptance

Participant acknowledgements advance that Membership's durable Cursor.
Anonymous viewers do not own a durable consumer Cursor: each public connection
installs a fresh authorized Public Projection Reset, then receives live updates.
The per-viewer relay acknowledges its synchronization barrier but does not send
`observation.ack` on the shared relay Membership. One viewer cannot consume
another viewer's reconnect position. Public viewing is not durable event
consumption or a participant recovery guarantee.

Run the complete local candidate from a clean checkout:

```sh
pnpm hosted:acceptance:local
```

The command starts one local Supabase stack, the Runtime and Controller, the
Platform BFF, the direct Hosted Gateway, the Agent Heist client, and a visible
fake OpenRouter. It then executes the frozen person, external-agent, House
Agent, and spectator story. It injects a browser disconnect and Runtime restart,
uses WebMCP read/wait/commit, reconciles the terminal Replay, and writes a
mode-0600 redacted evidence file under `.worldstream/evidence/`.
The scripted human proposes a reviewed plan to trigger the Pack's endorsement
Activation. The fake House Agent responds to that proposal and later commitment
events. The test does not assume an unsolicited House turn during briefing.
Its deterministic player strategy tests integration, not model reasoning quality.
If another participant advances the Room before a scripted human Action arrives,
the test revises only an explicit stale-state rejection that permits revision.
It reconnects, checks the current authorized offer, and uses a new Action ID.
Five failed attempts stop the test. An ambiguous submission or any other
rejection is not retried automatically.
The direct-push metric spans delivered state updates on one WebSocket
connection. Setup time, disconnected time, and time across a restart are not
added together.
An uncommitted working tree can be used to debug the gameplay checks, but its
evidence remains blocked. A passing release candidate requires the same clean
Git commit before the build and after verification.

The local substitute is accepted only on an explicit loopback address. The
production Platform and Fly startup paths reject every development substitute.

Run database capacity and concurrency tests before this acceptance command,
not against the same database while a match is running. The tests deliberately
exercise global admission limits. Finish other Rust builds and tests first,
too: a build can replace a Runner executable whose exact checksum is already
approved by the local stack. Changed executables are correctly refused.
A failed match retains its state and capacity
reservation for diagnosis; the command does not silently delete it or create a
replacement authority. Keep a private copy of the local Runtime and database
state before an intentional test-stack reset. Never reset the linked hosted
database to rerun a local test.

## Deployed acceptance

Create a fail-closed evidence template:

```sh
pnpm hosted:acceptance:deployed --write-template /private/path/deployed-input.json
```

Run the exact deployed story and replace each blocked item only with observed
evidence. Do not put credentials, cookies, handoffs, tickets, Room identity, or
private Projection data in the file. A passing deployed candidate must record:

- the exact Git commit, Vercel deployment, Fly image, Supabase schema head,
  Listing, Pack, Activity Client, and result projector identities;
- real GitHub OAuth;
- exactly one separately bounded real OpenRouter provider-call proof;
- more than five minutes of direct Fly push;
- Chrome desktop, iOS Safari, and ChatGPT desktop observations;
- disconnect, Catch-up, Fly restart, and re-entry;
- a Replay-verified exhibition result in Recent Results; and
- the complete negative-security cutline.

Then run the public endpoint checks and retain a new verified copy:

```sh
pnpm hosted:acceptance:deployed --verify \
  /private/path/deployed-input.json \
  /private/path/deployed-verified.json \
  --origin https://example.invalid \
  --public-id 00000000000000000000000000000000
```

The verifier checks the catalog, absence of a Vercel WebSocket route, GitHub
OAuth start, the terminal public Run, Recent Results, and public-data privacy.
It also compares the evidence with `/api/deployment`: Vercel's commit and
Deployment ID, the actual applied Supabase migration head, and the compiled
Listing, Pack, Client Release, and Result Projector digests. It reads the
configured Fly Gateway's `/version` separately. `gateway_revision` must be the
clean Git commit supplied as `WORLDSTREAM_DEPLOYMENT_VERSION` on Fly. Record the
immutable image digest separately in the existing deployment/checkpoint receipt.
These are deployment correspondence checks, not a cryptographic build attestation.
The verifier refuses placeholders, missing identity, mismatches, incomplete,
contradictory, oversized, or credential-like evidence. Real-device checks remain
operator observations; these HTTP probes cannot establish that a phone or a
browser agent completed the story.

## Small recovery loop

The creator's launch-status read repairs only an already-authorized frozen
operation. Genesis is recorded as soon as its evidence is available. A recorded
Activity Run does not mean entry is ready: participant entries remain unavailable
until the Host reports that the same Room Setup is complete. Retry does not make
another Room, reserve more capacity, or select different House agents.

Production also runs one protected daily `/api/internal/reconcile` sweep. Set a
separate random `CRON_SECRET` of at least 32 characters on Vercel. The sweep reads
at most ten candidates, repairs retained Genesis/setup correspondence, and checks
Replay-verified results. A failed candidate does not stop the rest of the batch.
Selection places unreleased active-Run reservations first (the MVP limit is ten),
then the least recently attempted work across all other categories. Missing
correspondence/results break ties ahead of published-result rechecks. A private
last-attempt timestamp rotates old or failing candidates across categories;
it is scheduling data, not proof of Room state or successful verification.
Public result reads continue to repair result evidence. This is an MVP fallback,
not a low-latency scheduler or a guarantee that all backlog clears in one day.

Local acceptance runs the negative/security prerequisite suites before importing
the pinned Runner or starting the match. It does not infer that those suites
passed merely because gameplay passed. Allow time for Rust tests, the appliance
smoke test, pgTAP, capacity concurrency probes, and database security checks.

## Release decision

The preview is ready only when both local and deployed evidence pass for the
same Git commit and deployment identities, the isolated checkpoint restore
drill passes, and no P0 or P1 authority, privacy, recovery, or result-integrity
defect remains open. Missing production credentials or infrastructure is a
blocked candidate, not a partial pass.

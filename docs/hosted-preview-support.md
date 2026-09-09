# Hosted hobby preview support and acceptance

This document defines what the first hosted WorldStream preview supports. It
also defines the evidence required before the preview is called ready.

For observed deployment results and outstanding checks, see the
[2026-09-07 deployment record](hosted-preview-status-2026-09-07.md).

## Platform-stability milestone — accepted 2026-09-08

The [accepted design decisions](activity-platform-design.md) require generic
setup, My games, entry, Run Re-entry, verified results, and safe repeat play.
Use Heist 0.3 with compatible client usability fixes as the first end-to-end
activity. Other Pack contracts must work without Heist branches in the shared
platform. Gameplay improvements, ranked leaderboards, and another hosted game
are outside this milestone.

The approved specification is
[IMO-187](https://linear.app/imom39a/issue/IMO-187/stabilize-the-generic-hosted-activity-platform-mvp).
The [implementation handoff](activity-platform-design.md#implementation-handoff--published-2026-09-08)
links each slice. IMO-184 remains the final deployed acceptance gate; publishing
the tickets is not evidence that this support contract has passed.

Qualification requires three consecutive local matches with scripted
participants and the fake provider, followed by one tightly capped real-LLM
match on the same deployed source revision. Test the actual browser journey,
including invitations, refresh, navigation away and back, restart, spectating,
result publication, and starting another game without operator cleanup. Keep
the existing supported-device, direct-WebSocket, and negative-security gates;
use non-Heist contract fixtures to verify the shared boundaries. Model victory
is not an acceptance condition. Local build and publication are supported;
CI/CD is not required.

The redacted local evidence uses
`worldstream/hosted-preview-acceptance-evidence/v2`. It contains a three-row
match matrix with per-match fake-provider call deltas, retained-history and
consumed-allowance observations, capacity release after each terminal result,
fresh setup after the third match, ordinary restart/re-entry status, and the
redacted receipt from the rendered Activity Client journey. The rendered
journey is one matrix match, not an additional unaccounted-for match.
`populated_recovery` is always recorded as `deferred_not_verified` under
[ADR 0026](adr/0026-defer-populated-disaster-recovery-for-the-hobby-preview.md);
an ordinary restart pass never upgrades that deferred recovery claim.

The commands and historical evidence described below are not automatically
proof of this expanded journey. The existing local protocol harness does not
render browser pages and must be extended or supplemented for these checks.
No passing stabilization evidence is claimed by this documentation change.

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

An ordinary network disconnect and a Runtime restart use different recovery
paths. After a network disconnect, the client can request a new stream ticket
with its current browser session. After a Runtime or Controller restart, each
signed-in participant must re-enter the same Run and redeem a new handoff for
the same seat. Re-entering does not create another Room or change its history.
Anonymous spectators reconnect to their public viewing address. In each case,
the client must receive fresh authorized synchronization before it allows
Actions. A previously displayed Projection is not proof of a live connection.

A retained House Agent restarts with the same Assignment, reviewed revision,
working directory, and allowance ledger. Restart does not reserve a new House
Agent or refill its budget. If its retained binding, executable, or credential
cannot be verified, restoration stays incomplete instead of replacing it.

### Completion and repeat play

The stabilization release must stop and fence completed House Runner units and
release their capacity safely without routine operator intervention. Platform
Run capacity and House Runner capacity are separate obligations. Preserve their
exact lifecycle evidence and consumed allowances; a stopped process, closed
browser, or elapsed timeout is not proof that an ambiguous Run ended. An
unresolved operation may temporarily block another launch. House spending
exhaustion remains an honest unavailable state, not permission to reset a
budget. The existing offline retirement procedure alone does not qualify this
repeat-play requirement.

### Disaster-recovery limit

[ADR 0026](adr/0026-defer-populated-disaster-recovery-for-the-hobby-preview.md)
defers populated cross-system restore qualification for this hobby preview.
Ordinary same-volume restart must preserve acknowledged Room history and
support authorized Run Re-entry. That requirement is not deferred.

Keep protected, paired, off-Machine captures under the existing backup cadence.
Without the populated correspondence verifier, these captures are unqualified
recovery evidence, not verified Hosted Recovery Checkpoints. Catastrophic
storage loss may require an explicitly operator-approved fresh preview and a
public history-loss disclosure. No current reset is authorized, no partial
restore may be served as coherent history, and no recovery operation may
repeat an ambiguous paid call or refill consumed spending allowances.

The earlier zero-history drill remains evidence only for its exact empty
installation. The populated-recovery gate remains unfinished and must be
tracked separately rather than marked passed for this release.

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

CI is a separate cost surface. A private GitHub repository uses the account's
included Actions allowance and can incur overage. Check its
[Actions budget and stop-usage setting](https://docs.github.com/en/billing/how-tos/set-up-budgets)
before repeated full builds. Do not enable larger paid runners or raise the
budget automatically. A test-run timing response is not an account billing
report.

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
Agent, and spectator story through a Node-based cookie, HTTP, and WebSocket
harness. It injects a participant-stream disconnect and Runtime restart,
uses WebMCP read/wait/commit, reconciles the terminal Replay, and writes a
mode-0600 redacted evidence file under `.worldstream/evidence/`.
This is protocol integration evidence. It does not render browser pages or
prove GitHub consent, mobile behavior, or real-provider access. Those checks
remain part of deployed acceptance.
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
too. Runner template r9 and later copy their checked executable bytes to an
owner-only, BLAKE3-addressed retained path before import. A later build may add
a successor path, but must never replace the bytes selected by an active
Assignment. Earlier r7/r8 templates retain their historical image-level
executable paths and are not safe to carry through an active executable
upgrade; do not claim that this new rule repairs them. Drain or fence those
legacy assignments before changing their image.

The current fresh-assignment chain is Runner template r15, Cooperative Planner
profile 16, Skeptical Auditor profile 15, and Agent Heist Listing 0.23.0. It
exists so the reasoning-disabled House model request has separate approval
evidence and an isolated Runner instance. The chain changes no Pack, client,
model route, allowance, projector, or gameplay rule. r14/0.22 and earlier
remain retained immutable identities.
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
- one tightly capped real-LLM match with recorded OpenRouter provider-call
  evidence within the existing per-assignment, aggregate, and key limits;
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
then the least recently attempted work across all other incomplete categories.
Missing correspondence and result-without-summary work remains retryable.
Once terminal evidence and a replay-verified indexed result are both recorded,
the Run is immutable platform history and is removed from the Host poll queue;
historical result reads use that stored result. A private last-attempt timestamp
rotates old or failing candidates across categories; it is scheduling data, not
proof of Room state or successful verification.
Public result reads continue to repair result evidence. This is an MVP fallback,
not a low-latency scheduler or a guarantee that all backlog clears in one day.

Local acceptance runs the negative/security prerequisite suites before importing
the pinned Runner or starting the match. It does not infer that those suites
passed merely because gameplay passed. Allow time for Rust tests, the appliance
smoke test, pgTAP, capacity concurrency probes, and database security checks.

## Release decision

The stabilization preview is ready only when the expanded local and deployed
journey checks above pass for the same Git commit and deployment identities,
protected paired state captures are retained, and no P0 or P1 authority,
privacy, ordinary-restart durability, result-integrity, or allowance defect
remains open. My games, safe setup recovery, browser re-entry, verified result
publication, and automatic completed-unit retirement must work without routine
operator repair.

Under ADR 0026, populated disaster-recovery qualification is explicitly deferred
and must not be reported as passed. That scoped exception does not excuse a
failed ordinary restart, a partial restore served as coherent state, or an
untested browser path. Missing production credentials or infrastructure is a
blocked candidate, not a partial pass. No existing acceptance receipt is
silently upgraded to satisfy this revised contract.

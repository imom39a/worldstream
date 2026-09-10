# Agent Heist: Mission focus release

## Scope

The user selected **A / Mission focus**: an illustrated, guided experience for
the existing Heist game. New multi-stage gameplay is a separate project.

This release changes the Activity Client, not the kernel or game rules. The
current Heist Pack stays at `0.5.0`. Scoring, deadlines, role ownership, legal
actions, House Agent revisions, and spending limits stay unchanged.

## Player experience

- A focused current move replaces the long action-form sidebar.
- Players choose named dossiers and plans, not internal IDs.
- The plan builder shows one decision at a time: way in, entry window,
  equipment, and way out. Each choice has its own illustration.
- The shared board, private intel, and rules are available when needed.
- The live clock remains a countdown. Opening help does not pause a live game.
- The debrief explains the five crew checks from the server result. This is
  not a model benchmark or an individual skill ranking.
- Separate guided practice is scripted and untimed. It needs no account,
  live Room, API key, or paid model call. Practice scores are not published.

## Authorization boundary

Live state starts empty and installs only an authorized Projection Reset.
Practice state is never copied into a live session.

Heist `0.5.0` publishes a fixed role-to-object mapping in its immutable rules:
Navigator has the route dossier; Insider has the entry-window dossier; Broker
has equipment and extraction dossiers. The client uses a reviewed descriptor
keyed to that exact Pack digest to label these objects. It contains **no clue
answers**. It is not dynamic target metadata returned by the server. Unknown
revisions have no inferred dossier targets.

The current inspection offer, authorized role, and already opened private
clues control the inspection affordance. Submitted moves use the current offer
and sequence; the server decides legality. Public spectators receive no
private intel or participant moves. Local drafts survive unrelated crew
updates but do not become Room truth before the server accepts an action.

## Release strategy

Publish immutable Client v9 paths at `/agent-heist-v9/`,
`/agent-heist-v9/hosted/`, and `/agent-heist-v9/practice/`. Listing `0.26.0`
selects the new hosted client for new Rooms. Retain Listing `0.25.0` and its
original v8 artifact bytes for existing Rooms. Keep all earlier supported
client paths and retained result-projector mappings.

Append the reviewed v9 declarations and Listing allowlist entry to the current
Fly installation. Do not reset its database, rebind existing Rooms, change
provider approvals, or add Machines for this UI release. The catalog-only Fly
image update preserved the existing database, Rooms, provider approvals, and
Machine count. The catalog migration adds immutable metadata only. A separate,
narrow corrective migration fixes expired-window pre-Host cancellation; it
does not change RLS, authentication, or the game rules.

## Evidence and status

Tracking: [IMO-215](https://linear.app/imom39a/issue/IMO-215).

The illustrated release was published on September 10, 2026:

- Website source: `25ebe1971e1926cf3cd843c861e07fefd699a18f`.
- Vercel deployment: `dpl_EiQSkBVzUBcQF6iGm37jSqs4NXkb` (Ready).
- Client release: `sha256:6ee6747c63cf0090a7549a1c505893c49307c475d1332eb4164c1170a97645d8`.
- Browser artifact: `sha256:9d030bf2f4c542f9fa6af00d378dc4d6083b7fee975252128a5c6cb11105ed5b`.
- Listing 0.26.0: `blake3:30ee53ed1ad230586f0f0ac20b3da442093c76bed568b3b7021b043800e240fe`.
- Catalog migration: `20260910120222_mission_focus_client_successor.sql`.
- The production website is v9 at deployment
  `dpl_EiQSkBVzUBcQF6iGm37jSqs4NXkb`, built from UI commit
  `25ebe19` (full revision above). All 32 v9/retained-v8 artifact files were
  fetched from production and matched their local SHA-256 bytes exactly.
- Fly image: `registry.fly.io/worldstream-preview:heist-v9-2dbdcfa`, digest
  `sha256:aa7670365483d599f728e9a843d5b30d82a09c9d010b9acfa3de05f731398490`.
  It is healthy with `/readyz` 200 and the Fly health check returned 1/1.
- Fly image source identities: base/native `c90a1fe252ecdba5471d0e613ea4f6de890f82ae`
  plus Controller `2dbdcfa801f58193704aad8a42c2fb8c26dd7754`.
- Six executable bytes are unchanged. The managed Host approved BLAKE3 is
  `b46f6de7c70a64088f8f23a9b5d370bd13995267318b51f7435e8f284a44ddbc`;
  House Runner Template 16 is retained.

[Open guided practice](https://worldstream-demos.vercel.app/agent-heist-v9/practice/).
New Listing 0.26 Rooms select the v9 hosted client. Existing Rooms keep their
pinned client; they do not silently upgrade to v9.

### Verified checks

- 55 Activity Client tests passed, plus client lint and production build.
- 153 platform tests passed; one database-concurrency test was skipped.
- 33 site tests passed, plus lint and production build.
- 32 hosted-runtime/local-development tests passed; two container-only checks
  were skipped. The catalog conformance check passed 25 Listings, 33 House
  revisions, and 25 allowlist entries; this is not a full Rust suite.
- All 15 real-container image tests passed, including entrypoint contracts,
  `/readyz` from inside the network-isolated 512MB container, installed v7/v8/v9
  binding identities, and same-volume restart readiness.
- Eight Vercel build-contract tests and ten client-workspace tests passed.
- Two catalog-overlay contract tests and 14 hosted browser-session tests
  passed. The new stale-state regression requires a fresh authorized Reset,
  disables actions while disconnected, and verifies that no move is replayed.
- Client identity checks passed for 14 Releases, three Distributions, ten
  Deployments, and 21 Bindings. This informative check does not replace Host
  approval or complete native-runtime qualification.
- All 24 v9 files and eight retained v8 files returned HTTP 200 from production
  and exactly matched the local SHA-256 hashes.
- Desktop and 390-pixel practice checks covered the illustrated choices,
  guidance, help, and correct/incorrect debriefs. Production phone smoke checks
  found no horizontal overflow, debug controls, failed requests, or calls to
  auth, Room, or model APIs. Practice is still a script, not an LLM evaluation.
- `/api/deployment` reported the reviewed source and client/Listing/Pack
  identities. Paired checksummed private capture completed. This does not verify
  populated recovery state. Gates reopened; no database reset, resize, or
  spending-limit change occurred.

The saved Launch Request `33262157-145a-4efd-a449-6100a47a873d` created
Run `69858e42-d136-4ad2-9893-d8f793ed952a` and Room
`01M25VMZFE3DBAE8P3GB9V5SA9`; two House agents and a human Navigator joined,
private route inspection was accepted, a share attempt received the honest
`stale_room_state` server rejection, and manual reconnect succeeded. The UI
reported `Complete`, Failure `0/5`, with no sealed choices. The crew did not
agree on a plan. This is not a winning-game or novice-usability result.

Retained House operation receipts prove seven submissions: five accepted
actions and two activity-domain rejections. Both House Agents had accepted
actions. The allowance ledger records 11 provider attempts, including four
invalid responses. These failures were bounded; no allowance was reset.

The [public result](https://worldstream-demos.vercel.app/runs/bb8824e2367cdfdbf69118d637b84ebf)
was Replay-verified at Room sequence 16 and appeared in My games and Recent
results. Both House reservations and the active-Run capacity were released.
An anonymous HTTP check verified the exact deployment identities, public
result, recent-result entry, OAuth redirect, and absence of private fields.
The Vercel WebSocket route remains absent; realtime traffic goes to Fly.

The human then created and abandoned people-only setup
`92bf5c5c-994c-4bc3-bcc9-3ef1308fb804` through the UI. It retained cancelled
history, released its capacity, and created no House reservation or Room.
No operator cleanup was needed.

At 15:01:50 UTC on September 10, after both paid tests, OpenRouter reported
USD `0.01365143` lifetime usage and USD `1.98634857` remaining on the dedicated USD 2 key,
with no reset. This is total key usage, not the price of this match. No
spending limit or credit-replenishment setting changed.

### Remaining playability blocker: hidden Room-head advances

The second bounded live test used Launch Request
`a57669e5-433e-4944-a7f2-2cd77cccf7df`, Run
`6365b41e-2c9e-45d5-aaeb-9023ab93564c`, and Room
`01M25X349BZ3D0PQNR61EZQDQG`. It reproduced stale dossier-open and clue-share
responses while the connection was live. After reconnect, route inspection
worked. The illustrated plan builder opened and retained a Canal selection
when moving to the entry-window cards. The phase then ended before a complete
plan was submitted; the client correctly discarded the unsubmitted draft.
The Room completed with 0/5, a Replay-verified result at sequence 16, and both
House reservations and Run capacity released. The
[second public result](https://worldstream-demos.vercel.app/runs/917adedf29616cda291c9b6ae8d01318)
is retained. No further paid test is needed before fixing freshness.

The initial explanation that every stale response was an in-flight race was
incomplete. Code inspection established a persistent freshness gap:

1. Another participant's private inspection can leave the human's entire
   authorized projection and offers unchanged. The Pack returns `Hidden`.
2. `prepare_transition_consequences` in `worldstream-core/src/room_commit.rs`
   emits no Observation Frame for `Hidden`. The live stream pushes frames only.
3. The browser advances its cached Room head on delivered observations,
   receipts, or a fresh attach/reset. It can therefore remain behind the
   authoritative sequence even while the WebSocket is healthy.
4. A submitted move passes the client's local check but the server rejects its
   old basis. Manual reconnect helps temporarily, not permanently.

The exact winning transition for each rejected human action has not yet been
correlated from receipts. The hidden-head gap itself is established in code.
[IMO-217](https://linear.app/imom39a/issue/IMO-217) tracks a deterministic
two-participant reproduction and a privacy-reviewed generic synchronization
fix. Do not remove authoritative stale checks, expose private data, or silently
replay a player move. Any required client successor must preserve published v9
bytes. This protocol/runtime correction is separate from new Heist gameplay.

IMO-215 remains open: live illustrated plan submission and sealed commitment
have not passed end to end. The cancellation fix, IMO-216, is complete. Wider
iOS Safari, anonymous live spectating, WebMCP, and live-Run restart qualification
are not claimed here. Populated recovery remains deferred.

The prior launch `3143` was cancelled successfully after applying SQL migration
`20260910132302`, commit `2dbdcfa`, and the 157-DB-test pass. The direct
`/my-games` reload path also returned 404 during earlier verification; that is
recorded separately as a hosting/router limitation and is not evidence of a
client or game-rule failure.
The follow-up [Room-service reliability patch](room-service-reliability.md)
adds the missing route and makes personal history resilient to maintenance
failure. Its release status is separate from the original v9 acceptance above.

### Local deployment lessons

Use a real, isolated `node_modules` tree for a local Vercel build. Links to a
different checkout can work locally while Vercel's file tracing omits required
packages. The first deployment failed to load `@noble/hashes`; an offline,
lockfile-pinned install inside this worktree fixed the packaged dependency.

Build and deploy from the repository root with the linked project settings.
Start with a fresh build-output directory; reusing a copied output directory
duplicated the daily reconciliation cron. Set the exact source revision for
both the local build and deployment environment. The pull command's empty
`VERCEL_GIT_COMMIT_SHA` value is not a valid release identity.

Every client import must include the exact retained Inspector fallback,
deployment, and release, even when the new client is the only new binding.
Review with `worldstreamctl init --preview`, then approve its exact digest.
The successful v9 import review was
`blake3:d561c5bfccdc9a4974dcb06519739c621ff00b0ec05d5bc03f623c94aac75753`.
No existing declarations were overwritten. Admission was closed during this
operation and reopened after readiness passed; Machine count, memory, House
Agent approvals, and spending limits stayed unchanged.

Artwork provenance and prompts are in
`clients/agent-heist-web/src/prototype-player/art/README.md` and
`entry-window-prompts.json`. Production uses bounded WebP derivatives.

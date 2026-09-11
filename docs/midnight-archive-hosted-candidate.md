# Midnight Archive internal hosted candidate

IMO-207 established the first solo candidate, IMO-211 added recorded session
expiry, and IMO-209 added the four reviewed roster choices. The current
revision is `worldstream.midnight-archive.internal-solo` version `0.4.0`, with
digest
`blake3:4c9a98ec044e9389b9a4e3d8a8f33a371dc6ed3991556037ada61cfba4bf718a`.
The `internal-solo` Listing ID is retained for identity continuity; version
0.4.0 adds bounded Room-busy synchronization recovery. Versions 0.1.0 through 0.3.0 remain available for exact
old-Run resolution.

The candidate remains reviewed and unlisted. It offers exactly four starts:
solo, Mira, Jonah, and Mira plus Jonah. Every option includes one human lead,
uses `standard-v1`, and starts with sixteen turns and three shared power
charges. The supplied companions are included for the capped exhibition and
begin each Run without memory from another Run. The Listing permits no human
invitation, BYO agent, substitute House revision, arbitrary prompt, or changed
Room configuration.

The current Listing pins:

- Pack Revision
  `blake3:aea45a1c056c4a7da744be33d38df672499062fd2548302fae43adc49b833b07`
  in Bundle
  `blake3:de3cd1d9fa45087b69cb107a663596305864c350f620fe4d7260e0341214d47a`.
- Activity Client v14 Release
  `sha256:2040145dc2017df23f810a410b9cc9c36ce56da7e242dcaaf28feeda2d099886`
  with artifact
  `sha256:e4c0b6c75297d3320b70f492c8034d733f94dd01d082e1b36838cda7649dc1cd`.
- Mira House Agent Revision
  `blake3:7e0b07b386009d509d605c9efdbe491a035f219d10ef7ebebc6f71e99461cdde`
  and Jonah House Agent Revision
  `blake3:b88da2260f391c91593996c5913961469619b53a9e457c5ea0783cd3cac59db0`.
- Result Projector 0.2.0
  `blake3:93bdc21b4b09ec6e7c1ed7a11df80d984e2f80175e9fae01143f3a00c65d4a17`
  and Public Projection v5
  `blake3:c8045ca0762df97d4e82488f562f7a69eea2b480e41f06889eef3dff133086d0`.

The Listing identity alone grants no execution authority. A Host Operator must
still approve and activate the exact Pack, client, Profiles, Runner Template,
House Agent revisions, executable, credential route, and allowance. Capacity
or health failure for a selected companion fails the start without choosing a
different companion or silently falling back to solo.

Before launch, the Listing discloses that an active expedition expires 24 hours
after the Host records Activity Start. Closing a tab, disconnecting, or
re-entering does not pause or reset that deadline. Expiry is a terminal state
with no gameplay Outcome. It preserves discoveries, resources, completed
specialist work, and the original Membership correspondence while publishing
no public result. Terminal reconciliation retires Run and House capacity once;
My Games can reopen the retained private debrief for the original participant.

The authenticated `/api/catalog/internal` route exposes the candidate only when
its exact digest appears in `internalCandidateListingDigests`. Its launch
control remains unavailable until `hostedActivityAvailable` verifies the exact
Listing is recognized by the live Gateway and Controller. Final launch
admission remains authoritative for Pack, client, roster, House approval,
allowance, and capacity readiness; a missing or mismatched dependency fails
closed. Production construction opts in only when
`WORLDSTREAM_INTERNAL_CANDIDATE_LISTING_DIGEST` equals the exact reviewed
Listing digest above. Omitting the variable leaves the production catalog
public-only; every other value stops production startup.

`pnpm hosted:dev` reads `config/hosted/internal-candidates.json`, approves and
installs the exact Bundle, imports the v14 client, and configures the local
availability check. An empty candidate list still starts the public-only
library. Retained Listings resolve through their original clients and
projectors; they are not current discovery entries.

Qualification evidence is recorded in
`packs/midnight-archive/evidence/qualification-imo-209.md` and
`packs/midnight-archive/evidence/qualification-imo-211.md`. The evidence spans
the portable Component Host, SQLite Room, WebSocket streams, recorded timers,
Replay, result projection, local Postgres RPCs, concurrency, and durable House
allowance accounting. It remains local composed evidence: no deployed Host or
paid external-provider completion has been claimed.

Run `node scripts/verify-midnight-archive-hosted.mjs` against `pnpm hosted:dev`
for the current signed-in solo launch/play/private-debrief journey through the
v14 hosted client. New journey evidence writes under
`.worldstream/evidence/imo-209`; historical IMO-207 and IMO-211 evidence remains
under its original directories.

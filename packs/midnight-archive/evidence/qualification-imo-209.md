# IMO-209 four-roster qualification

Midnight Archive Listing 0.3.0 offers exactly four reviewed choices: solo,
Mira, Jonah, and Mira plus Jonah. Every choice retains the human expedition
lead, the Standard scenario, sixteen turns, and three shared power charges.
The two supplied companions use exact immutable House Agent revisions. No
option permits a substitute agent, arbitrary prompt, provider route, or Room
configuration.

The browser and Listing describe this as a capped, unranked exhibition. The
supplied agents are included at no charge, each Run starts without memory from
another Run, and the same limits remain visible during live play and in the
terminal debrief. Registration grants no Host approval. An operator must still
bind and activate the exact Profile, Runner Template, executable, credential,
and Host installation.

## Acceptance evidence

| Requirement | Deterministic evidence |
| --- | --- |
| Four clear choices, human lead, specialist descriptions, free exhibition | `hosted-archive-listing.test.mjs` compares the migration's canonical Listing and House documents with their reviewed sources. `midnight-archive-roster-qualification.test.ts` resolves all four production options and their exact seat sets. `hosted-catalog.test.ts` checks the four public descriptions and omits private configuration and identities. |
| Exact House identities, policies, route, Profiles, Runner Template, and allowances | `hosted-archive-house-identities.test.mjs` checks Mira and Jonah's distinct revisions, behavior policies, fixed Granite/DeepInfra route, zero-data-retention rule, exact Profile and Template revisions, and bounded allowance. Its approval test binds exact installed evidence without activating it. `midnight_archive_four_rosters.test.sql` checks the registered revisions and copied Assignment allowances. |
| Exact capacity/readiness before start; no substitution | `midnight_archive_four_rosters.test.sql` completes exact singleton reservations and Assignments for every supplied option, rejects a full-crew launch when the global Runner gate cannot fit both seats, and leaves no partial reservation or Assignment. `midnight-archive-roster-qualification.test.ts` rejects a supplied-roster failure before freeze, Host authorization, Genesis, or Run creation. Actual production Host readiness remains a live qualification item below. |
| Formation, live, and debrief disclosure; no cross-Run memory claim | `hosted-catalog.test.ts` checks prelaunch no-charge, fixed-budget, expiry, and fresh-memory language. `MidnightArchiveClientView.test.tsx` tests **“labels a live companion Run with the included exhibition limits and fresh-memory boundary”** and **“attributes only completed specialist work in the terminal debrief”**, including the same exhibition and fresh-memory terms. |
| All four missions; retained request, seats, Membership, receipts, and allowance on return | `current_bundle_completes_all_four_rosters_and_replays_exactly` runs the exact Listing-pinned Bundle through the portable Component Host, SQLite Room, participant and companion WebSockets, full extraction, and authorized Replay for solo, Mira, Jonah, and both. The production pgTAP repeats every exact launch request and rereads its selected claims, Assignments, receipts, and allowances after exact v3 freeze. The activity-neutral BFF test **“hosted re-entry preserves the original participant Membership and supports spectator admission”** checks Membership continuity. `platform_house_fill.test.sql` checks **“fully retired House runners retain exact original principals, Assignment IDs and reservation receipts for re-entry”** and **“retirement never changes retained Assignment allowance or reservation history.”** `hosted-roster-fixture.test.mjs` checks **“Host evidence preserves consumed allowance and exact bindings while omitting authentication material.”** |
| Selection/claim concurrency, pre/post-Genesis failure, absent optional Roles, Heist regression, no uncertain Action replay | `verify-platform-formation-concurrency.mjs` races sixteen immutable option requests, sixteen identical launches, eight claims, sixteen House starts, and sixteen exact House selections; each retains one winner or one exact outcome. The production Archive pgTAP proves absent Roles never receive a claim, reservation, or Assignment and proves v3 create/freeze behavior while `platform_roster_options.test.sql` retains the v2 create/freeze contract. `hosted-formation.test.ts` tests **“post-Genesis provisioning retries the same setup before offering Run entry,” “recorded Genesis is never converted into a provisioning abandonment,”** and **“a lost first Host request is recovered with the original authorization and request.”** `hosted-rendered-browser-journey.test.mjs` tests **“resumed rendered journey reads its existing authorized Route claim without replaying Inspect,” “rendered commitment retries only after the explicit stale Room reconnect boundary,”** and **“rendered commitment never retries a non-stale failure.”** `midnight-archive-roster-qualification.test.ts` also checks the retained Agent Heist Listing's established fill contract. |
| Visible optional and main-mission assistance with unchanged core budget | Pack tests **“every supported starting roster is retained at Genesis with the same budget,” “every supported roster completes the same fifteen-turn route with its entire starting crew,” “Jonah opens the hatch for one charge; Mira's ordinary method costs two,”** and **“Mira's two Vault assay steps disclose no finding before eligible completion and replan after sample”** prove the authored distinctions. The exact all-four Component witness checks sixteen initial turns, three initial power, visible completed specialist work, complete main mission, extraction, and exact Replay. |

The criterion map is deliberately composed from boundaries that own each fact.
For example, the BFF owns re-entry Membership continuity, Postgres owns frozen
selection and Assignment/receipt retention, the Host ledger owns consumed
allowance, and the Pack owns gameplay effects. A mock formation test alone is
not treated as proof of all those behaviors.

## Verification record — 2026-09-10

`pnpm archive:rosters -- --node-only` passed under Node 24.18.1:

- hosted TypeScript contract: 23 passed;
- hosted package and exact artifact tests: 67 passed, with three explicit
  environment-dependent skips;
- controlled roster/HTTP-provider and rendered-journey tests: 27 passed;
- platform catalog, formation, BFF, and production-roster tests: 83 passed;
- Pack tests and deterministic transcript: 86 passed;
- Activity Client lint and tests: 113 passed.

The exact database and Component commands are kept separate so a developer can
run them against the required local services without repeating every Node test:

```sh
pnpm archive:rosters -- --database-only
pnpm archive:rosters -- --component-only
```

The database phase runs the production v3 Archive pgTAP, the retained v2 roster
suite, the retained Agent Heist House-fill suite, and the real local PostgREST
concurrency harness. The Component phase passes the Listing-pinned Pack Bundle
path to one ignored-by-default exact Room witness. It does not execute the
Activity Client artifact; the separate v13 checks cover that browser release.

Both phases passed under Node 24.18.1. The database phase passed 124 pgTAP
assertions plus its real concurrency witness. The exact Component test
`current_bundle_completes_all_four_rosters_and_replays_exactly` passed once in
972.71 seconds, with eight unrelated tests filtered out. It completed solo,
Mira, Jonah, and full-crew Rooms through the portable Component Host and
authorized Replay.

The full database suite also passed 524 assertions across thirteen files after
the creator-closure and private-terminal migrations were composed. It verifies
that closing work wins while in progress, healthy terminal Rooms remain
privately reopenable, dependency failures stay distinct, expiry publishes no
public result, and reconciliation retires capacity once.

Rust and Python companion adapters accept the current v5 participant
Projection while retaining the reviewed v4 contract and rejecting later or
mismatched schemas. Rust tests build model context from the genuine current v5
Pack fixture (SHA-256
`770053fea77175ff0cf780efd1275f21e4f57b84fbf33cbe4184f3952de894db`)
without exposing the session deadline. Host catalog tests prove that the
retained thirty-three House revisions plus Mira and Jonah total thirty-five
unique identities.

Headless browser verification passed for current Heist v9, current Archive
v13, and the retained client paths. The signed-in local journey also passed
against the v13 hosted surface: guest discovery stayed hidden, solo formation
was idempotent, the ten-turn technical route extracted the authentic ledger,
the private terminal record published no result, capacity retired, and the
original participant re-entered the same Room. It then created and closed a
retained Heist setup through the current closure route without browser errors.
This used the visible local-development identity and fake provider described
below; it is not deployed evidence.

`pnpm hosted:dev --check` passed from a cold retained-server start. The local
launcher tolerates only the Controller-unavailable and incomplete
Runtime-restart reports while that one start is still converging, for at most
thirty start attempts separated by one-second pauses. Its browser-artifact
builds run in production mode; the subsequent client correspondence check
retained all exact current Release digests.

## Immutable candidate identities

- Pack Revision:
  `blake3:aea45a1c056c4a7da744be33d38df672499062fd2548302fae43adc49b833b07`
- Pack Bundle:
  `blake3:de3cd1d9fa45087b69cb107a663596305864c350f620fe4d7260e0341214d47a`
- portable Component:
  `blake3:7781388b52a2d335d07f5cee7658810bfb036ebe0e3b5c293459c7d38385af0c`
- Activity Client v13 artifact:
  `sha256:6d7fcea16b1305034af5a4f46214fb22b686d6aa831d53513ebe7d33ee342c4e`
- Activity Client v13 claims:
  `sha256:1e75a44c426a98165d206d2abb5b2ce2dbd32140b97558984910c088f9c9ca3c`
- Activity Client v13 conformance evidence:
  `sha256:2b80f66672e03ce5f9386088a826b282345282637a310eb63ad16ee964dbf0f0`
- Activity Client v13 Release:
  `sha256:240ac1b94e9e89c0eb5a059ba85d116b807258cfd5917bc0dd879139de1b35bb`
- Mira House Agent Revision:
  `blake3:7e0b07b386009d509d605c9efdbe491a035f219d10ef7ebebc6f71e99461cdde`
- Jonah House Agent Revision:
  `blake3:b88da2260f391c91593996c5913961469619b53a9e457c5ea0783cd3cac59db0`
- Profiles: `house-midnight-archive-mira` revision 1 and
  `house-midnight-archive-jonah` revision 1
- Runner Template: `openrouter-house-archive` revision 1
- Result Projector 0.2.0:
  `blake3:93bdc21b4b09ec6e7c1ed7a11df80d984e2f80175e9fae01143f3a00c65d4a17`
- Listing 0.3.0:
  `blake3:cc1c92ebc6ba7cccc9474186ff8107cf97f6bd0ce2676c6d1a2aa203c2a62d35`

## Qualification limits

This is local composed evidence. It does not yet prove a deployed Host or an
external provider-backed completion for these exact Mira and Jonah revisions.
The pgTAP creates transaction-local test approvals and controlled successful
receipts; those records do not establish an operator approval, installed
executable health, credential readiness, or real Runner capacity. The
Component witness authenticates deterministic scripted companion Actions
through real WebSockets; it does not start the production managed Runner
subprocess or call the configured OpenRouter route. The HTTP provider test uses
an authenticated loopback substitute and makes no paid external call.

No Fly, Vercel, or deployed Supabase journey was run for Listing 0.3.0. Live
provider latency, response quality, actual allowance consumption, all-four
deployed return journeys, and exact Host activation remain unproven. IMO-209
must not be marked release-qualified from this document until that live
evidence is attached.

# Reviewed Roster Options

ADR 0027 extends the immutable Activity Listing Revision's launch-input contract.
The Listing Revision document remains `worldstream/activity-listing-revision/v1`;
its new input declaration is `worldstream/launch-input-schema/v2` with
`accepts: "roster_option"`. Retained `launch-input-schema/v1` / `none` documents
continue accepting exactly `{}` without rewriting their artifacts or Rooms.

A v2 declaration contains one to sixteen `roster_options`, each with an
`option_id`, reviewed `label`, nonempty `seat_ids`, exact `configuration`, and
`house_agent_assignments`. The default is one existing option ID. An assignment
contains only a predeclared `seat_id` and an exact allowlisted
`house_agent_revision_digest`. At most two distinct House revisions can be
assigned, with no duplicate seat or revision. Options must retain every required
seat. Every selected seat must resolve to an account or its pinned supplied
agent. Option seat order does not override Listing seat order.

The browser receives option labels, opaque seat keys, permitted creator seat
keys, and the supplied-agent count. It sends `roster_option: "<option_id>"`
alongside the existing bounded create-launch fields. `fill_mode` must agree
with whether that option supplies agents. It cannot send configuration, Roles,
Principal IDs, prompts, provider/model routes, code, or setup documents.

## Immutable formation

The Launch Request stores canonical `{ "roster_option": "<option_id>" }`
bytes before its creator Seat Claim. Existing account/idempotency uniqueness
and immutable-row protection bind those bytes to that Listing Revision. A
changed option under the same key conflicts, including after restart.

`selected_listing_seats_v2` resolves a transaction-local view of the Listing
seats for creation, reads, invitations, claims, fill selection, setup freeze,
Genesis reconciliation, and private launch material. It preserves the Listing
and retained rows. Excluded seats are absent; selected seats must all be filled.
Pinned supplied seats cannot receive account invitations or claims. Their
candidate allowlists contain only the exact assigned revision; account seats
cannot be filled with a different supplied agent.

The existing locked House Fill Operation retains its claim window, candidate
and exclusion evidence, random bytes, reservation operation IDs, and receipts.
For a pinned option each supplied seat has a singleton compatible candidate;
random bytes do not choose a replacement. Existing account and deployment
capacity gates still apply. Assignment publication remains one transaction
after every receipt succeeds. A crash after the first receipt preserves that
receipt and leaves no partial Assignment set. A terminal failure retires the
same Launch Request without rerolling. Ambiguous Host state retains capacity.

Both Rust and TypeScript independently rederive the selected roster and exact
configuration from frozen Launch Request input. The database additionally
checks the selected configuration and seat list when freezing setup. The
existing `launch-<LaunchRequestId>` Room Setup Operation identity and all
Launch/roster/setup hashes continue to fence retries and observed Genesis.
The Host receives only the selected seats, with the original Roles and seat
requiredness. Browser availability checks only selected account-controlled
seats; supplied seats are governed by exact House readiness and capacity.

## Qualification and handoff

`fixtures/hosted-contract/valid/roster-options-listing.json` is a shared schema
fixture for Rust, TypeScript, and database tests. Its old House identities are
schema references, not Archive execution approvals.

The separate real-Host fixture lives under
`config/hosted/fixtures/roster-options/` and is rendered by
`scripts/hosted-roster-fixture.mjs`. It uses the retained Archive 0.1.0 Pack with
solo and supplied options, separate qualification Profile/Runner/House
identities, and a visible controlled provider. It is not part of the production
catalog. `WORLDSTREAM_LOCAL_HOSTED_FIXTURE_DIRECTORY` is accepted only with
explicit development configuration, visible fake-provider mode, nonproduction
execution, and loopback Controller/daemon addresses. Reading fixture artifacts
grants no Pack, Profile, Runner, credential, or client approval.

The BFF has an explicit server-owned `reviewedActivities` injection for local
qualification. Internal discovery still requires authentication, exact Listing
opt-in, and exact dependency availability. Production browser requests cannot
supply a catalog or fixture path.

Focused validation:

- TypeScript contract tests cover all four schema-fixture choices, missing and
  extra seats, required-seat retention, exact House mapping, and authority-field
  injection. Rust verifies the same fixture and derivation restrictions.
- Authenticated BFF tests check labels, private configuration, exact input
  storage, unsupported options, excluded creator seats, and incompatible fill.
- The stateless coordinator test loses readiness after Genesis and restarts
  with the same v2 input, operation, roster, setup, and Run.
- `platform_roster_options.test.sql` checks account privacy, option immutability,
  quota refusal, excluded invitations, pinned selection, partial-receipt crash
  recovery, terminal reservation failure, and whole-set capacity refusal.
- Existing formation and House-fill tests remain regression requirements;
  `verify-platform-formation-concurrency.mjs` checks retained concurrent claim,
  capacity, and selection behavior.

IMO-209 supplies the final Mira/Jonah definitions and successor Archive Listing.
This extension does not modify the existing Archive Listing, current candidate
identity, Pack gameplay, model policy, or retained Heist fill artifacts.

The SQL migration replaces complete existing formation function bodies. When
integrating later lifecycle work, preserve that work and reapply the selected
seat resolver at each boundary; do not replace newer close/abandon semantics
with an older function body.

## Verification record — 2026-09-10

Executed with Node `24.18.1` from
`/Users/vinothshanmugam/.nvm/versions/node/v24.18.1/bin`:

| Check | Result |
| --- | --- |
| Hosted TypeScript contracts | 22 passed |
| Hosted Rust contracts, serial | 24 passed |
| Hosted contract Clippy (`--no-deps`, warnings denied) | Passed |
| Platform suite | 161 passed, 1 optional live test skipped |
| Common activity-library suite | 32 passed |
| Selected browser availability tests | 2 passed |
| Roster coordination database suite | 31 passed |
| Existing formation database suite | 68 passed |
| Existing House-fill database suite | 63 passed, including terminal re-entry regression |
| Real PostgREST formation concurrency | Passed, including 16 competing option requests |
| Platform and activity-library typechecks | Passed |
| Local platform database schema lint | No errors |
| `git diff --check` | Passed |

The local migration was exercised through direct SQL against the retained
local installation; no database reset or production deployment was performed.
Database fixture changes roll back, and the concurrency harness removes only
its own namespaced fixture rows. The migration file still needs the normal
integration/application process.

The real-Host fixture browser journey is a separate required qualification;
these contract and coordination results alone do not claim that journey passed.

### Terminal private re-entry regression

The first real supplied-agent fixture reached terminal reconciliation and
retired its House runner, then the authenticated Launch read returned 503.
The private launch-material reader included only `succeeded` reservations;
retirement changed that capacity state to `released`, omitting the original
Assignment from frozen setup reconstruction before any Host request.

The reader now retains receipt-backed `succeeded` and `released` Assignments.
A second boundary was exposed by retrying the original Run: `readHouseFill`
rejected the valid `released` reservation state in its TypeScript adapter.
The adapter now reads that terminal state without changing it; its regression
failed against the old parser and also checks that unknown states remain
rejected. Direct Host status and Genesis reads both returned 200 for the
original launched Room throughout this second failure.

This preserves historical setup identity; it does not authorize execution or
alter capacity, Memberships, Assignment allowance, or retirement receipts.
The legacy House-fill database suite compares the complete private launch
material before and after partial and complete terminal retirement and checks
that both capacity units remain released. These assertions failed against the
old reader and pass with the fix. The change is confined to the private reader
in the roster migration, preserving later close/abandon state transitions.

Focused follow-up checks: platform 161 passed (one optional live skip),
platform typechecks and build passed, serial Host launch tests 18 passed and
House runner tests 11 passed. The real-Host
harness separately retains the failed boundary and verifies the original
terminal Run before repeating the complete fixture journey.

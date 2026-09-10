# IMO-205 deterministic qualification

The schema v4 Pack accepts optional speech through the existing
`submit_companion_plan` Action's required `dialogue` string. Empty means none.
Full accepted utterances are Activity State facts and canonical events with
frozen original speaker/lead Membership audiences; they do not become shared
crew evidence or permissions. Private-source knowledge suppresses nonempty
speech until a structured share has resolved. No chat endpoint, human text
control, new Invocation or hidden conversation memory was added.

The managed House boundary selects reviewed Mira/Jonah policies, accepts only
an exact current v4 Projection or Observation and one offered plan Action,
and independently checks schema, task/opportunity revisions and dialogue
privacy/bytes before submission. Ten attempts per assignment, one in flight,
input/output/aggregate accounting, exact route/privacy controls, retry fencing
and timeout charging remain unchanged. Provider failure creates no game fact.

## Measured complete requests

The retained House fixture is generated through actual Pack helpers: both
sources shared, both companions present, four accepted utterances per speaking
companion at exactly 192 bytes after double JSON encoding, accepted three-step
plans, and a fresh planning opportunity. It includes the real complete v4
Action schema/digest, actual fifteen-second eligibility window, full offer ID
and Head overhead. Reset and latest-frame request bodies are byte-identical.

| Scenario | Mira request bytes | Jonah request bytes |
| --- | ---: | ---: |
| Standard | 11,294 | 11,159 |
| Low Reserve | 11,310 | 11,175 |

The existing cap is 12,000 bytes. Responses containing three typed steps and a
full 160-byte raw utterance measure 579 reply-content bytes for either Role,
under the unchanged 1,000-byte cap. Oversized input is rejected before ledger
reservation or dispatch. Invalid speech after dispatch consumes the paid
attempt; replaying that attempt never dispatches it again.

## Commands and results

All JavaScript commands used Node 24.18.1. Python used
`uv run --project sdk/python --python 3.14.7 python -m pytest`.

- `pnpm --filter @worldstream/midnight-archive pack:test`: 74 passed, with
  deterministic conformance. New tests cover UTF-8, escaping, C0/unpaired
  surrogate rejection, no-record rejection/empty behavior, original audiences,
  replacement Memberships, participant/historical/public/operator/final views,
  oldest-first bounded eviction, and structured-share privacy fencing.
- `WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl" pnpm --filter
  @worldstream/midnight-archive pack:prove`: passed using the production
  portable Component Host. Exact receipt is in
  [production-proof-0.1.0-dialogue.json](production-proof-0.1.0-dialogue.json).
- `cargo test --locked -j 1 -p worldstream-server --test
  midnight_archive_component_room full_crew_resolves -- --test-threads=1
  --nocapture`: passed, 297.66 seconds. Both companions submit authenticated
  plans with full dialogue, the lead receives compact recommendations, turns
  retain human commitment, and terminal authorized Replay exactly reconstructs
  accepted dialogue and actual crew work without a provider.
- `pnpm archive-client:lint`, `pnpm archive-client:test`, and
  `pnpm archive-client:build`: passed; 107 client tests. Advice is literal React
  text, labeled recommendations; HTML/Markdown is not interpreted, and no human
  free-text control appears. The reader rejects extra/private fields and byte
  violations.
- `uv run --project sdk/python --python 3.14.7 python -m pytest
  examples/midnight_archive -q`: 126 passed, including independent dialogue
  bounds and existing failure/concurrency/late-reply behavior.
- `cargo test --locked -j 1 -p worldstream-studio-supervisor --lib`: 138 passed,
  one explicit network diagnostic ignored. The House subset has 26 passing
  tests and that same ignored diagnostic. The final fixture-only normalization
  to actual canonical fifteen-second offer timestamps passed all eight Archive
  House tests again.
- `node scripts/verify-activity-clients.mjs`: passed for 23 Releases, 3
  Distributions, 19 Deployments and 28 Bindings.
- `node scripts/verify-activity-client-browser.mjs`: passed for exact current
  and retained client artifacts.
- `WORLDSTREAM_ACCEPTANCE_REUSE_BINARIES=1` with the exact Bundle and v11
  Release paths and `WORLDSTREAM_MIDNIGHT_ARCHIVE_PROOF_MODE=crew-live node
  scripts/verify-midnight-archive-browser.mjs`: passed. The real browser/Runtime
  expedition completed in eleven Activity Turns with two power remaining,
  desktop/phone layouts, separate Memberships, one external deterministic
  Invocation per companion, full crew extraction and authorized Replay without
  rerunning either policy. Provider calls: zero. Exact receipt is in
  [browser-proof-0.1.0-dialogue.json](browser-proof-0.1.0-dialogue.json).
  Normal cleanup removed the ephemeral stack and its processes.
- `git diff --check`: passed.

`cargo clippy --locked -j 1 -p worldstream-studio-supervisor --lib -- -D warnings`
was blocked by the existing `clippy::map_unwrap_or` diagnostic in
`crates/worldstream-core/src/agent_heist_lobby_v5.rs:164`, outside these changes.

## Immutable candidate identities

- Pack Revision: `blake3:26c51f969dc7949fb42556d013eeec555ae83dc9f28cb582c03f0541e420e776`
- Pack Bundle: `blake3:4325846eb61ee4b821d4e7d95a0a3e9c5f78aeaddbc2649cf308b33efcc68812`
- Component: `blake3:f991741654552d77734f54ff7aa8752c3c24bb45a00c90091691f12f3b3059b0`
- Client v11 Release: `sha256:44f4c66dabdc4cbf6eccfb1bad7443735038bc0cf1ea938c73e427bae10bb49f`
- Client v11 artifact: `sha256:c28fe026025357cda94391e1abaad4f1dba24c18ebc7418a3f3f2efb639f8a06`

Older immutable Pack/client artifacts remain retained. The local v11 binding
is separate from hosted catalog approval and does not migrate existing Rooms.

## Remaining live qualification

No paid/live provider call was made. The exact Archive-compatible Agent
Profiles, Runner Templates and published House Agent Revisions are an IMO-209
integration dependency. No authorized local provider allowance was established
for these named assignments here. Actual provider latency, attempt usage and
contribution for each companion therefore remain unqualified. Deterministic
schema/size/Room/browser results do not establish live reliability, economics,
fun, Host approval or hosted release readiness.

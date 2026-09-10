# Local roster qualification fixture

This fixture proves the launch-input-v2 solo/supplied distinction over the retained
Midnight Archive v3 Pack and v10 client. It is not a production Listing, Mira
policy qualification, or paid-provider experiment. The supplied seat has Role
`mira` but is explicitly named a controlled qualification assistant.

`scripts/hosted-roster-fixture.mjs` derives validated exact Listing and House
revisions from the pinned source. `render-hosted-roster-fixture.mjs` writes their
immutable documents, an Agent Profile and an Archive-compatible Runner Template:

```sh
node scripts/render-hosted-roster-fixture.mjs OUTPUT_DIRECTORY RETAINED_MANAGED_HOST_PATH RAW_BLAKE3
```

The Host fixture directory is `OUTPUT_DIRECTORY/host`; imports are under
`OUTPUT_DIRECTORY/imports`. Retain and hash the actual executable before rendering.
Re-rendering identical files is safe; different bytes at an existing path fail.
Rendering grants no Host approval or runtime availability.

Use only the guarded local Host fixture registry and development BFF reviewed
activity injection. Add the exact Listing to the internal allowlist, register
its canonical platform documents, approve the exact fixture House revision for
the local Host, and import its Profile/Template. Continue to use the retained
Archive Pack/client and existing terminal projector; do not change their bytes.
The browser participant is the human lead; the House-only optional seat does
not need a browser surface.

Start `scripts/hosted-roster-fixture-provider.mjs` instead of the ordinary fake
provider for this qualification. It preserves Heist fallback behavior and all
existing development-only authentication/network guards. Its Archive response
uses the exact offered task/opportunity and current authorized Records facts.
It never obtains hidden Pack truth. It accepts only retained v3 observations.

The explicit local startup hook retains the existing local Host and database,
imports/registers only the additional fixture, and starts the injected catalog
and controlled provider. It does not reset existing Runs:

```sh
WORLDSTREAM_ROSTER_FIXTURE_QUALIFICATION=visible-local-only node scripts/hosted-dev.mjs --native-profile=release
```

After that stack is ready, run in another terminal:

```sh
WORLDSTREAM_ROSTER_FIXTURE_QUALIFICATION=visible-local-only node scripts/verify-hosted-roster-fixture.mjs
```

The browser witness checks authenticated selection, immutable option/idempotency
lineage, one Genesis, zero/one House Assignment, a controlled accepted plan with
two committed steps, honest no-ledger extraction, original Membership re-entry,
and released Run capacity. Its evidence is written under
`.worldstream/evidence/imo-208-roster-fixture`; no evidence is fabricated if a
dependency is missing. The script does not clear database or Runner state.
The supplied journey also records exact imported Profile/Template/executable
digests, one completed bounded provider attempt, the durable retirement receipt,
and unchanged consumed allowance after terminal re-entry. It excludes credential
references and authentication tags from evidence. Failed stages retain partial
evidence and a screenshot instead of producing a passing receipt.

Run helper/provider tests with:

```sh
node --test scripts/hosted-roster-fixture.test.mjs scripts/hosted-fake-openrouter.test.mjs
```

These focused tests are not the real-Host journey. Crash/uncertain-Genesis and
concurrent claim/capacity cases remain in the formation suite. The provider is
deterministic test execution; this witness does not assess live model quality,
four-option production availability, or browser behavior across a Host crash.

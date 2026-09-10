# Midnight Archive internal hosted candidate

IMO-207 is implemented and locally verified. This registration is a development
candidate, not a production approval or deployment.

The initial Listing is `worldstream.midnight-archive.internal-solo` version
`0.1.0`, with digest
`blake3:fbe603c57e281038a0dca6cb4066d3dfcfdbf420d905e5a70893c1bdefbfc27b`.
It pins the authored-scenarios Pack Revision
`blake3:f40e0a287fcaac6e6bc56629d361ede079d6c3c60aa0068caa3a451dfb8c0b64`
and the v10 hosted client release
`sha256:9be6d6548f2d62baba805ec461b27fbd1021ed0d61fbd0e8ffd1d779616bc534`.
The only seat is the human expedition lead; configuration is fixed to
`standard-v1` and no launch input is accepted. Anonymous viewing and result
publication are disabled.

The BFF's authenticated `/api/catalog/internal` route includes a nonpublic
Listing only when its exact digest appears in the deployment's
`internalCandidateListingDigests`. Its launch button remains unavailable
unless `hostedActivityAvailable` verifies the exact approved Pack, client
release, surface and Client Binding. A missing checker, negative result, or
service error fails closed. Public discovery remains unchanged. Production
construction does not supply this candidate opt-in. `pnpm hosted:dev` reads
`config/hosted/internal-candidates.json`, approves and installs its exact Pack
for the local Host, imports the exact hosted client, and provides a fail-closed
availability checker. That checker compares installed and running selectable
Pack inventories, approved default Participant Bindings, ready Deployment,
exact Release and every served artifact file. It grants no approval itself.
An empty `candidates` array starts the public-only library; its empty serialized
allowlist requires no candidate checker configuration and leaves retained
Heist discovery available. Nonempty malformed allowlists still fail closed.

The exact Result Projector interprets the Pack's minimal Public Projection
`worldstream.midnight-archive/public-projection/v3`. Reconciliation records
terminal evidence and retires capacity without copying the Outcome into an
Indexed Activity Result. My Games shows `terminal_private` and retains entry
through the original Run Membership Correspondence; the private debrief stays
inside the Activity Client's Participant Projection.

The Host uses current authorized public evidence for Listings with disabled
publication. Publishable Listings retain their existing Replay path. This is
necessary because portable Packs can declare a distinct historical Projection
schema: the current Replay response omits that identifier, and the existing
Host adapter reconstructs its hash with the current schema. Private terminal
disposition does not require that publication proof. General publication for
portable Packs with distinct historical schemas remains a separate protocol
follow-up; no Replay mismatch is treated as verified.

Validation completed for this slice:

- Platform tests: 158 passed, one live acceptance test skipped.
- Demos tests: 32 passed; platform and demos TypeScript checks passed.
- Local pgTAP: 25 tests passed for candidate registration and My Games,
  including unhealthy and conflicted private terminal suppression.
- Generated artifact correspondence and exact bundle Public Projection schema
  checks passed; local security advisors found no error-level issues.
- Hosted startup/availability tests: 20 passed, including mismatched Pack,
  pending inventory, disabled Binding, revoked Deployment and altered served
  client bytes. BFF tests separately verify unavailable candidates reject new
  launches.
- Rust hosted-contract tests: 22 passed; seven focused Host artifact,
  result-source and retained publishable-source tests passed.

Run `node scripts/verify-midnight-archive-hosted.mjs` against the local stack
for the real signed-in solo launch/start/play/debrief acceptance journey.
The witness covers retry identity, private terminal re-entry and retained
Heist setup. It uses the visible local development sign-in substitute, real
Supabase, BFF, Gateway, Runtime, installed Component and immutable v10 client.

The fresh journey passed at `2026-09-10T09:44:46.174Z`:

- Launch `a14d7d85-c900-43a9-ac9b-27f2709ee8d4`; Run
  `cbf5aafe-616e-4b1e-b331-3ff91693ee17`; Room
  `01M25B8F8GHDNHC7R39KP6MVE1`.
- Anonymous catalog excludes the candidate; the same signed-in library offers
  Archive and retained Heist. Solo setup omits role/fill controls.
- Repeated create/start retained one Run. Ten committed turns recovered Violet
  Ledger and extracted the lead with six turns and zero power remaining.
- My Games showed `terminal_private`, no public result, and reopened the same
  terminal Room with unchanged original Participant correspondence.
- Database checks confirmed one terminal evidence row, no indexed result or
  public identifier, and no remaining active capacity reservation.
- Retained Heist discovery, role/fill setup and pre-Genesis cancellation passed;
  browser execution reported no page errors.

Local evidence is `.worldstream/evidence/imo-207/journey.json` and
`terminal.png`. `recovered-original-journey.json` also records successful
completion of the first retained Room after the integration fixes. No Room or
database reset was used. The local Client Host now includes the exact configured
Gateway HTTP/WebSocket origins in its connection policy, so the streamed
Participant Projection can connect through the same hosted browser handoff.

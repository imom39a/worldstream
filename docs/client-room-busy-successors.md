# Room-busy Activity Client successors

These immutable releases carry the shared `HostedLiveSessionController` fix from
`a694a74` and `9f240f4`: a retryable `room_busy` during initial synchronization
closes the unsynchronized socket, obtains a fresh one-use ticket, and retries
within the owning attempt's original deadline. Superseded attempts cannot close
or clear a newer attempt's socket or synchronization callback. An Action is
never replayed by this recovery path.

The source build uses Node 24.18.1, pnpm 11.19.0 and the frozen workspace
lockfile. Only the client release and Listing version/client reference advance;
Pack, Room configuration, House Agent, Result Projector and roster identities
remain the same as each predecessor.

Agent Heist v11 and Listing 0.28.0 are now retained predecessors. The current
source release is documented in [Agent Heist commitment receipt successor](heist-commitment-receipt-successor.md).

## agent-heist client v11 / Listing 0.28.0

- Build tree: `sha256:ce7c86d24832158536df90384d140ae5aec6482eb654ab70c1551effc9e62093`.
- Release claims: `sha256:b2824fb8cc5172aedcc68f55d3ac43a6c3bf49f54bc15a87e862773395dcdb6a`.
- Conformance evidence: `sha256:bcda50d1a8a301b9af666a53b6e03d2c98ed452c28e9be452ad4098b16f008f8`.
- Release: `sha256:40d452a04b096d3a0952f99b877f094f90c6e4a84063d3212265b5c2b97306ee`.
- Listing: `blake3:d738402a5acb404dead979c21002fa02d95f4c1e89f6d4c47e617a6c6be27bc3`.

Entrypoints: `/agent-heist-v11/` and
`/agent-heist-v11/hosted/`. Guided practice is at
`/agent-heist-v11/practice/`.

## midnight-archive client v14 / Listing 0.4.0

- Build tree: `sha256:e4c0b6c75297d3320b70f492c8034d733f94dd01d082e1b36838cda7649dc1cd`.
- Release claims: `sha256:8df35b03f23efddc28d0f15ba2a16689114bcb4995123dd4c6582ef41dd68ca5`.
- Conformance evidence: `sha256:f458a2fcbb5530e148a4294b50ed72f9ade70046639d0345aa02a592157af0ab`.
- Release: `sha256:2040145dc2017df23f810a410b9cc9c36ce56da7e242dcaaf28feeda2d099886`.
- Listing: `blake3:4c9a98ec044e9389b9a4e3d8a8f33a371dc6ed3991556037ada61cfba4bf718a`.

Entrypoints: `/midnight-archive-v14/` and
`/midnight-archive-v14/hosted/`.

## Retention and verification

Before either source build, the repository's `activityClientBuildDigest` helper
verified the frozen predecessor trees:

- Heist v10: `sha256:e93fb44b3b88b689ccde120e2625bb16571b138d6b1d3aa310ac9a99ae9bfcaa`.
- Archive v13: `sha256:6d7fcea16b1305034af5a4f46214fb22b686d6aa831d53513ebe7d33ee342c4e`.

Those trees, predecessor release/evidence documents, Listings 0.27.0 and 0.3.0,
and all previous bindings remain exact. Both local and Vercel artifact hosts
serve the frozen predecessor bytes at their original paths. Archive v14 is
also frozen under `config/activity-clients/artifacts/`; Heist v11 is retained
there too and checked against its release digest during installation.
New bindings are eligible explicit choices, so importing them does not replace
an existing Host default. Hosted Listings require their exact new release.

The Vercel demos build installed and verified both new client trees and all
retained hosted mounts. Focused packaging/workspace tests passed; the two
Linux-image-only entrypoint tests require a locally built appliance image.
The data-only seeds `supabase/catalog/agent-heist-0.28.0.sql` and
`supabase/catalog/midnight-archive-0.4.0.sql` were each applied twice in one
local Supabase transaction. Exact Listing-to-client correspondence was queried
and verified before rollback. No historical migration or catalog seed changed.

The reproducible conformance lane is `pnpm activity-clients:verify`, which now
includes the shared SDK's attempt-fencing, deadline, cancellation, and retry
regressions. These are informative conformance identities, not Host approval
or production deployment evidence. Production must coordinate the Fly release
inventory and allowlist, Vercel client artifacts and candidate environment
identity, and both catalog seeds, then record deployed acceptance separately.

Local verification on September 10, 2026:

- Frozen-lockfile install, all client lint/builds, generated-artifact check,
  exact release verifier, and the exact Vercel demos build passed.
- SDK: 57 tests; Heist: 57; Negotiate: 11; Archive: 113; Inspector: 59.
- Packaging/development/workspace Node checks: 98 passed, two image-only checks
  skipped. Hosted platform: 183 passed, one database-gated check skipped.
  Demos: 46 passed. Documentation: verifier/lint/build and nine tests passed.
- Built-client browser acceptance passed, including initial ticket failure,
  explicit reconnect, expired-session re-entry, and mobile layout.
- Rust Activity Client: five tests passed. Supervisor client bindings: nine
  passed. Participant-handoff compilation passed. Its executable emitted no
  test output for over three minutes on this macOS host; termination was
  initiated at that boundary. It emitted partial passing results during
  termination, then exited with SIGTERM before a complete test result.

The aggregate `activity-clients:verify` lane therefore did not complete. Its
participant-handoff execution, full Rust workspace lane and composed Archive
acceptance remain for Linux CI; no passing result is claimed for those steps.

# IMO-204 Pack and Room qualification

The Pack keeps expeditions playable when a companion does not reply. It does
not accept provider-health assertions or own provider attempts, concurrency,
timeouts, Session replacement, or model selection; those remain Runner concerns.

Two demonstrated gaps were corrected:

- An accepted personal or sibling-companion edit could leave an obsolete
  planning opportunity open after its Head changed. Accepted edits now cancel
  the exact pending Timer before applying the edit. Rejected edits preserve
  the window, and accepted standing plans retain their eligible work.
- Planning deadlines used fixed millisecond formatting, which emitted
  noncanonical timestamps such as `.000Z` and truncated nanoseconds. Deadlines
  now preserve the exact fifteen seconds with minimal canonical fractional
  precision. The stale-plan comparison preserves that same precision.

Validation used Node 24.18.1 and the production portable Component Host:

- Pack tests and conformance: **62 passed, 0 failed**. Coverage includes both
  Roles' serialized windows, exact Timer generations, cancellation/expiry
  races, late/malformed/ineligible replies, no implicit retry or work, retained
  plans, explicit defer/wait/follow/regroup, no-allowance continuation, restored
  Membership eligibility, partial extraction, and nanosecond/rollover boundaries.
- [Production proof](production-proof-0.1.0-unavailable-companions.json): passed.
- `cargo test --locked -j 1 -p worldstream-server --test
  midnight_archive_component_room -- --test-threads=1 --nocapture`:
  **6 passed, 0 failed**, 1021.37 seconds. These lanes cover solo, Mira, Jonah,
  full crew, explicit partial extraction, and unavailable companions. Every
  lane compares terminal facts with authorized Replay. The solo lane also
  admits six superseded retained bundles under their original identities.
- The unavailable lane uses an injected HostClock and the real Runtime
  scheduler. It verifies cancellation, exact expiry without a turn or implicit
  work, stale-Head and absent-Action-Offer rejection, original-capability
  reconnect for both agents, and a no-allowance partial extraction attributed
  only to executed follow/regroup work.

Qualified immutable identity:

- Bundle: `blake3:0ddcd385d0b250f9ec5a285df1783fbae45a685a90fcf69703026787959b540f`
- Revision: `blake3:d398d13df28f50edcc271aa8f6ffa75f1c26eca1851ac78215aa8e0f6017d8e5`
- Component: `blake3:326eaf7db7474a9484f8e53a6398f111077e8bf4f41d0b4fc79590a2a0ac6d0d`

This evidence qualifies the Pack and Room boundary. It does not qualify a
particular Activity Client Release or provider-backed Runner execution.

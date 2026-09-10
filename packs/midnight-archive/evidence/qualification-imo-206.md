# IMO-206 authored-scenario qualification

The current Pack freezes one of two closed configurations at Genesis:
`standard-v1` starts with three shared power charges and `low-reserve-v1`
starts with two. They use the same five-location map, sixteen-turn budget and
Action surface, but pin different candidate attributes, evidence values and
hidden authentic ledgers. The initial reserve and complete nineteen-operation
cost schedule are projected as authoritative facts. No narrator, client or
Runner can supply arbitrary scenario data or reroll discovered truth.

The authoring validator rejects unknown scenario IDs, additional fields,
inconsistent source/candidate facts, a source that identifies only one
candidate, or an intersection that does not identify exactly one authentic
candidate. Reducer tests cover all four Standard authentication/access pairs,
the three feasible Low Reserve pairs, and both orderings of the infeasible
Low Reserve powered-verifier plus ordinary-service combination. The
sixteen-turn Low Reserve evidence/agreement witness completes both optional
objectives with zero power remaining. Both scenarios also run through solo,
Mira, Jonah and full-crew roster fixtures.

Validation used Node 24.18.1 and the production portable Component Host:

- Pack tests: **69 passed, 0 failed**. `pack:check`, `pack:build`,
  `pack:inspect` and `pack:prove` passed.
- [Production proof](production-proof-0.1.0-authored-scenarios.json): passed
  with three Roles, three private views, an accepted Action, a declared
  rejection and retained-old-revision admission.
- `cargo test -p worldstream-server --test
  midnight_archive_component_room -- --test-threads=1`: **7 passed, 0
  failed**, 1407.21 seconds. Every lane uses the real Component Host and
  compares terminal facts with authorized Replay. The new Low Reserve lane
  follows both disclosed sources, spends both charges on the ordinary hatch,
  rejects an unaffordable verifier without mutation and extracts the uniquely
  recommended ledger in thirteen turns. The other lanes cover every roster,
  simultaneous reservations, complete and partial extraction, unavailable
  companions and retained v9 admission.
- Activity Client tests: **105 passed, 0 failed**; TypeScript checking and the
  production build passed. The client rejects Projection drift, renders the
  selected scenario and initial reserve, and derives every displayed cost,
  staged-action check and local power threshold from the Pack-projected closed
  operation schedule.
- `node scripts/verify-activity-clients.mjs` passed for **22 Releases, 3
  Distributions, 18 Deployments and 27 Bindings**. Browser artifact acceptance
  passed for current and retained clients. The v10 build tree is retained and
  served at its exact release path.
- All six real-browser modes defined by
  `scripts/verify-midnight-archive-acceptance.mjs` passed against the exact
  Pack and v10 client identities. Standard agreement completed in eleven
  turns; both optional objectives completed in fifteen; Low Reserve completed
  both optional objectives in all sixteen turns with zero power; Mira reached
  a recoverable partial extraction after one external Runner invocation; the
  full crew extracted the verified ledger in eleven turns after one invocation
  each; and the unavailable-companion witness bounded delayed, rejected and
  missing provider dispositions before reconnecting without rerunning policy.
  Every mode checked phone and desktop layout, authorized Replay or reconnect,
  and private-authenticity non-leakage. The settled-binary full-crew run also
  recovered from two exact transient `502` observations through the client-only
  retry policy.
- The deterministic companion adapter accepted the v3 participant Projection
  contract: **38 tests passed**, and Ruff passed. This is provider-free
  compatibility evidence rather than LLM qualification.

Qualified immutable Pack identity:

- Bundle: `blake3:8083201f2d1d6a1afdaab8e6759287a3aeacc1a7908d85d2b758af272c150b47`
- Revision: `blake3:f40e0a287fcaac6e6bc56629d361ede079d6c3c60aa0068caa3a451dfb8c0b64`
- Component: `blake3:aa124b20667020510de26eb1658f54876b49a6354d0950052c804f6a5338e0a1`
- Production transcript: `blake3:b5bfb634ad87eae8f2da73361121d6eb05c6eaf30c8992b79a3b83e3d0142ff6`

Qualified exact Activity Client v10 identity:

- Artifact: `sha256:068474029614d80990ce7919c4ead4eb863b4355fc37e9904413955a9b680027`
- Release claims: `sha256:5de47858d446f7926ce8a2b37b71309598b42b950b3f0ee9d24baacce634655a`
- Conformance evidence: `sha256:f6f939f4e52c39fbe918f5ec3436d45cb759c12958c9f62ebd963c1d561f47a3`
- Release: `sha256:9be6d6548f2d62baba805ec461b27fbd1021ed0d61fbd0e8ffd1d779616bc534`

This evidence qualifies authored scenario rules and their local Pack/client
execution. It does not establish hosted discovery, model quality, session
length, enjoyment, retention or an endless content cadence.

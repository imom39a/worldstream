# WorldStream A202 adapter

`worldstream-a202-adapter` is the transport-neutral Application Integrator
boundary for the compatibility profile frozen in
[`docs/negotiate.md`](../../docs/negotiate.md). It is not a protocol server and
does not add authority to the Room Kernel.

The adapter:

- accepts one configured A202 transaction/session pair;
- retains `application/a202-commercial+json` as opaque exact bytes;
- independently checks canonical form, A202 SHA-256, ES256 signature purpose,
  signer binding, and supplied historical/current key status;
- records a separate WorldStream BLAKE3 identity for the exact bytes;
- keeps signed A202 `expected_sequence` inside an ActionEnvelope and the
  WorldStream Room Head outside it;
- permits Room-Head-only retry with the identical signed bytes;
- returns `RebuildAndResign` when either logical A202 Head changed;
- verifies exact resolver responses against an authenticated Host Stimulus
  Source without performing network access; and
- constructs the two-section Negotiate proof package.

It deliberately contains no HTTP client/server, private key, signer, model,
prompt, strategy, storage, Room mutation, or A202 conformance claim. External
clients sign; `worldstreamd` remains the sole venue authority; the Activity Pack
decides the deterministic domain result.

The upstream source contract and known interoperability limit are recorded in
[`docs/a202-operated-profile-research.md`](../../docs/a202-operated-profile-research.md).
The release-artifact invitation and strict external-receipt boundary are
documented in
[`docs/a202-peer-interoperability.md`](../../docs/a202-peer-interoperability.md).

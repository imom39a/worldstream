# WorldStream Negotiate

`worldstream.negotiate` is a WorldStream Activity Pack. It implements the operated, single-session A202 formation profile frozen
in [`examples/negotiate/design.md`](../../negotiate/design.md) through the public TypeScript
Pack SDK and CLI.

The source Pack has no network, filesystem, clock, entropy, key-custody, or
process authority. The CLI compiles it to a WASI-free WebAssembly Component
with exactly the five `ActivityPackV1` exports and no imports. Strategy,
credentials, prompts, models, tools, and commercial signing keys remain in
external Runners.

## Public build path

From the repository root:

```sh
pnpm --filter @worldstream/official-negotiate pack:check
pnpm --filter @worldstream/official-negotiate pack:test
pnpm --filter @worldstream/official-negotiate pack:build
pnpm --filter @worldstream/official-negotiate pack:inspect
WORLDSTREAM_PACK_HOST=target/debug/worldstreamctl \
  pnpm --filter @worldstream/official-negotiate pack:prove
```

`pack:test` reads the independent, machine-readable contract at
[`examples/negotiate/oracle/fixtures/corpus-v1.json`](../../negotiate/oracle/fixtures/corpus-v1.json).
It proves all nine success checkpoints, the persisted restart point, the exact
signed-expiry outcome, semantic rejection cases, four-Role Core invariants,
the six-persona privacy matrix, and all 30 ordered cross-persona privacy
mutations. The Rust oracle is never linked into the production Pack.

`pack:build` reruns that suite, compiles the Component, writes the canonical
`.wspack`, and emits receipts under the ignored `.worldstream/` directory. A
release is complete only after the production Rust Bundle Verifier and
Component Host execute its retained Core golden transcript. The final exact
bytes are retained under `releases/<explanatory-version>/` with their physical
BLAKE3 digest in the filename; a digest-named file is immutable.

## Official portable releases

Negotiate is an official Pack, but it is not embedded in `worldstreamd`.
Operators approve, install, and select its exact portable Bundle before they
start the Runtime. This keeps the immutable base Runtime Distribution separate
from the operator-owned Pack inventory.

The current selectable 0.2.0 release is
[`worldstream-negotiate-83453ea9641f8b16e9b96bf536c5ee932611611817458f130d8b77c7b93ff9a8.wspack`](releases/0.2.0/worldstream-negotiate-83453ea9641f8b16e9b96bf536c5ee932611611817458f130d8b77c7b93ff9a8.wspack).
Its physical bundle digest is
`blake3:83453ea9641f8b16e9b96bf536c5ee932611611817458f130d8b77c7b93ff9a8`;
its semantic revision is
`blake3:651a04711a61bbdb263da5869a48d9829bc37b3be315042404587b00d52c127c`;
and its exact Component digest is
`blake3:9b1632d43012d4a4b046467b27c05e3c4b7f8abf8bc0ef2ca0c3bb762de957ed`.
The complete production proof is
[`evidence/production-proof-0.2.0.json`](evidence/production-proof-0.2.0.json).

The 0.1.0 release is retained for existing Room lineage and must not be
selected for new Rooms:
[`worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack`](releases/0.1.0/worldstream-negotiate-9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5.wspack).
The project contract continues to target the mutable `candidate.wspack`
authoring output; normal build/prove commands never overwrite this retained
digest-named release subject.
Its physical bundle digest is
`blake3:9033a1aa10ca37c301660b7427d79c4e71d7af59006bc7d51edc4b470c8c2db5`;
its semantic revision is
`blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589`;
and its exact Component digest is
`blake3:17ab497995271c36b4ddc753eb85519cd9210a1c0c26e97b29c35ce5c95d8aff`.

The retained 0.1.0 public two-phase proof has Core transcript
`blake3:5ca09a58fdb40dc35116130e42e6ce6f5f99a39d52c36a4c148c23386a29bcb2`
without changing the Component, descriptor, revision lock, or semantic
revision. The complete machine-readable cross-language and production proof is
[`evidence/conformance-v1.json`](evidence/conformance-v1.json).

## Fixture security boundary

The checked corpus uses deterministic fixture proofs so two independent
implementations can compare exact bytes. Those fixture proofs are not
production signatures and do not replace the pinned A202 schemas, mandates,
resolver checks, public-key algorithms, or offline evidence verification.

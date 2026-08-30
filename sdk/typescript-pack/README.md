# TypeScript Activity Pack authoring

This workspace contains the first supported non-Rust authoring path:

- `@worldstream/pack-sdk` owns deterministic JSON bytes, callback result types,
  and the code-first `ActivityPackDefinition` contract.
- `@worldstream/pack-cli` owns strict compilation, capability diagnostics,
  generated WIT and wrapper code, ComponentizeJS/Jco quirks, deterministic
  tests, canonical `.wspack` construction, inspection, and proof receipts.

The pinned toolchain is TypeScript 5.9.2, Jco 1.32.1, and ComponentizeJS
0.22.0. The CLI always disables `stdio`, clocks, random, HTTP, and fetch-event;
the resulting Component must have exactly the five WorldStream exports and no
imports.

## Golden path

```sh
npx @worldstream/pack-cli@0.1.0 new vendor-negotiation
cd vendor-negotiation
npm ci
npm run pack:check
npm run pack:test
npm run pack:build
npm run pack:inspect
npm run pack:prove
```

`pack:prove` intentionally fails closed with `WSP-PROVE-001` until a production
Component Host prover is configured. A partial machine-readable receipt is
still written to `.worldstream/proof-receipt.json`. A successful production
proof writes `.worldstream/first-success-receipt.json` only after it creates a
real two-Role Room, observes two private views, records an accepted Action and
a declared rejection, and proves an older installed revision remains runnable.

The proof command compiles the Component exactly once. If Core reports that the
draft corpus needs its authoritative transcript identity, the CLI rewrites only
`golden-corpus.json`, `conformance.json`, and the bundle manifest, verifies that
the Component, descriptor, revision lock, and semantic revision did not change,
then proves those exact finalized bytes again. A second finalization request or
any identity drift fails closed.

The generated `.wspack` is a canonical uncompressed ustar archive. Authors do
not edit WIT, schemas, codec metadata, revision hashes, dependency evidence, or
bundle manifests.

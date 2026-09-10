# @worldstream/pack-cli

The supported TypeScript Activity Pack toolchain. It provides `new`, `check`,
`test`, `build`, `inspect`, and `prove` while hiding Jco, ComponentizeJS, WIT,
and canonical bundle mechanics behind owned WorldStream diagnostics.

## Create a project

The offline, code-first path remains the canonical starting point:

```sh
worldstream-pack new ./my-pack
```

First-release prompt assistance is an optional way to create that same fixed
project schema. It accepts an OpenAI-compatible Chat Completions endpoint and a
strict four-field JSON blueprint; the provider cannot supply files, paths,
commands, packages, or executable rules.

```sh
export WORLDSTREAM_PACK_OPENAI_ENDPOINT=https://provider.example/v1/chat/completions
export WORLDSTREAM_PACK_OPENAI_MODEL=provider-model-id
export WORLDSTREAM_PACK_OPENAI_API_KEY=provider-secret

worldstream-pack create ./my-pack --prompt \
  "Create a two-party calibration-services price negotiation"
```

The command returns `review_required` and the path to a private staged project.
It does not create the destination, install packages, run a shell command,
build, approve, or install a bundle. Inspect the staged files, then promote the
exact reviewed bytes with the receipt's explicit confirmation command:

```sh
worldstream-pack create ./my-pack --confirm <review-id>
```

After promotion, both creation paths use the identical commands:

```sh
npm ci
npm run pack:check
npm run pack:test
npm run pack:build
npm run pack:inspect
npm run pack:prove
```

The endpoint may use unencrypted HTTP only on loopback. Provider response
bodies, request transcripts, endpoint, model, and API key are never written to
the project, bundle, receipts, daemon, evidence, or normal diagnostic output.
The fixed prompt blueprint may only select the supported starter and supply its
display name and description; authors review and edit the code after promotion.

## Archive contract qualification fixture

The repository includes a non-Heist fixture that exercises optional Roles,
declared ExternalInput, audience schemas, and timed offers. Build and finalize
it with the production prover before running the ignored Host admission test:

```sh
WORLDSTREAM_PACK_HOST="$PWD/target/debug/worldstreamctl" \
  node sdk/typescript-pack/packages/pack-cli/dist/main.js prove \
  sdk/typescript-pack/packages/pack-cli/fixtures/archive-contract

WORLDSTREAM_ARCHIVE_CONTRACT_BUNDLE="$PWD/sdk/typescript-pack/packages/pack-cli/fixtures/archive-contract/releases/archive-contract.wspack" \
  cargo test -p worldstream-component-host \
  host::tests::archive_contract_bundle_rejects_invalid_payload_and_offer_boundaries_before_callbacks \
  -- --ignored --exact
```

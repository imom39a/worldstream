# IMO-54/60 Agent Heist compatibility identity

Date: 2026-08-21

## Result

The checked-in compatibility manifest now resolves the exact embedded Agent
Heist revision instead of leaving its six digest fields empty. The values were
read from the validated Rust registry and independently checked where a
source-byte formula exists.

The manifest remains `release_ready = false`; resolving an implementation
identity does not manufacture release artifacts or cross-platform evidence.

## Verified identity

- revision: `blake3:755aa7a88b8236d951da297e700ab41501d091a0ee1585547e90d8a46da95bfe`
- descriptor: `blake3:890f6a9e809e15247e728f84606fa511c02c634a36be0d185662107e11683bdc`
- executor artifact: `blake3:aa25d60ab8a60c2d52994f7d77d0219f8a543a4bc3fa34bed8362289ce208916`
- schema bundle: `blake3:6dfb0613326e19f27ede58048ccb4804d6d5832b59e3d0c5ee697bd6f7df1a4f`
- codec bundle: `blake3:67b814baf1511b6c29ffe9862eeeb2b130988972c9c3a2c2ab6ff1f7018a6b30`
- golden corpus: `blake3:8ec7045a92264300aa014879bb4137db7f730b9338eb9fe0cde455e26b4fd073`

`cargo xtask compat verify` now checks the full Agent Heist row against the
embedded retained/selectable registry, in addition to the existing Counter
rows. The Wave 6 Python audit independently recomputes the executor source
digest and requires all six BLAKE3 fields to be resolved and well formed.

## Parent checks

```text
cargo xtask compat verify
  passed

cargo clippy --locked -p xtask --all-targets -- -D warnings
  passed

cargo test --locked -p worldstream-core --lib \
  agent_heist_registry::tests::registry_golden_transcript_is_fixed
  1 passed

python3 -m unittest -v tests/manifest_evidence_wave6.py
  4 passed

python3 scripts/manifest-evidence-wave6.py --json
  Agent Heist manifest identity: resolved
```

## Scope

This closes the manifest-identity contradiction previously recorded for
IMO-54 and strengthens IMO-60's fail-closed compatibility verification. It
does not by itself prove a retained old non-selectable Heist revision,
cross-platform execution, signed release artifacts, or final release
readiness.

# IMO-59/60/61 supply-chain Luna lane

This lane audits whether the current environment can produce an identity-backed
Sigstore/SBOM/provenance package. It is deliberately bounded and disposable.
The helper is [`scripts/supply-chain-evidence.sh`](../../scripts/supply-chain-evidence.sh)
and its tests are [`tests/supply_chain_evidence.py`](../../tests/supply_chain_evidence.py).

The helper has three explicit modes:

- `audit` computes SHA-256 for each supplied subject and verifies that the
  generated checksum set covers exactly those subjects. It only reports local
  tool availability.
- `keyless` checks cosign, ambient/explicit OIDC identity, and SBOM/provenance
  generator prerequisites. Missing prerequisites return a non-zero result and
  no signing attempt is made.
- `local` can sign the disposable checksum set with an explicitly supplied
  local cosign key. The report remains `release_evidence: false` and
  `identity_backed: false`; it is not a release artifact.

The script does not read or write `compatibility.json`, `compatibility.toml`,
release digests, or compatibility manifests. Its JSON schema is
`worldstream/non-release-supply-chain-audit/v1`, and its report explicitly
contains empty `manifests_read` and `manifests_written` arrays.

Release integration is separate and detached: the release gate expects
`release-manifest.json` v2 to bind exact artifact and release-gated evidence
paths to SHA-256 values. The detached manifest additionally hashes
`SHA256SUMS`, SPDX, and SLSA, while excluding only the Sigstore bundle from its
signed subject set. Cosign verifies the detached manifest itself, and only the
Sigstore bundle path is retained as verification material; its digest is not
recorded anywhere in the manifest. No archive or evidence
digest is written into the embedded compatibility manifest, avoiding a
self-referential hash.

## Environment audit

Observed on 2026-08-21 in the parent macOS arm64 environment:

- cosign v3.1.3 is installed at `/opt/homebrew/bin/cosign`.
- No ambient GitHub Actions OIDC request URL/token or explicit Sigstore
  identity token was present.
- `syft`, `trivy`, `grype`, `bomctl`, `slsa-verifier`, and
  `slsa-provenance` were not installed.
- Therefore a real identity-backed signature/SBOM/provenance bundle cannot be
  produced in this environment. A local cosign signature, if a caller supplies
  a local key, is only a bounded local-key artifact.

The exact commands and outputs for the audit and tests are recorded in
`/tmp/luna-supply-chain-report.txt`. No output from this lane is release
evidence, and no manifest digest was filled.

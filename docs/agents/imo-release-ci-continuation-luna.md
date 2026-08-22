# IMO-59/60/61 release/CI continuation — Luna

Date: 2026-08-21
Checkout: `/Users/vinothshanmugam/code/agent-streamer`
Policy: shared dirty checkout preserved; compatibility manifests, compatibility release digests, `release_ready`, Rust/application code, and Linear were not edited.

## Linear acceptance scope

The authoritative rows were fetched from the `WorldStream` project:

- `IMO-59` — Ship supported native and OCI distributions: native Linux/Windows archives and backend probes; non-root read-only-root OCI with explicit persistent volume; fail-closed permissions/secrets/filesystems; SDK/UI/runtime identity; macOS source-only quickstart; no ARM64, Windows container, installer, cloud credential, or HA artifact.
- `IMO-60` — Automate compatibility, security, and supply-chain gates: fast/pre-push/hosted manifest-driven cells; Linux/Windows/OCI and PostgreSQL harness coverage; migration/transfer/restore/snapshot/kill/privacy/capability/lease/filesystem/telemetry tiers; checksums/signature/SPDX/SLSA verification; provider-neutral evidence; drift failure.
- `IMO-61` — Publish final v0.1 evidence: packaged Counter/Heist parity, forced-termination and recovery outcomes, privacy/capability/lease/retention matrix, Linux reference performance and one-hour soak, and no undeclared compatibility gap.

All three rows remain `In Progress`. The checked-in policy is still `manifest_kind="specification"`, `release_ready=false`, and `validation_policy="fail_closed"`; unresolved release fields remain authoritative blockers.

## Confirmed defect fixed

The OCI minimal-CI route previously ran only `scripts/package.py oci-context --dry-run`, so a malformed Dockerfile could pass the manifest cell without any BuildKit parse check. `scripts/gates.py` now:

1. passes a pinned syntax-only Alpine base reference to the OCI dry-run; and
2. runs `docker buildx build --check --platform linux/amd64` against `packaging/oci`.

The pinned base is a template-validation input only. It is not written to either compatibility manifest and is not release evidence. In CI (`--ci`), unavailable Docker is a hard failure; local non-CI runs retain an explicit incomplete skip.

## Commands and results

All commands ran without changing the shared checkout’s unrelated dirty files.

| Command | Result |
| --- | --- |
| `python3 tests/package_smoke.py` | PASS; deterministic tar/zip/source/OCI-context/wrapper/security checks |
| `python3 -m unittest tests.manifest_evidence_wave6` | PASS; 4 tests |
| `python3 scripts/gates.py fast --offline --report /tmp/worldstream-fast.json` | PASS; 13 checks, 0 failures |
| `PATH="$HOME/.nvm/versions/node/v24.18.1/bin:$PATH" bash scripts/macos-source-quickstart.sh` | PASS; 60 Python tests, 31 UI tests, TypeScript/Vite build; source-only, no signed/notarized binary |
| `PATH="$HOME/.nvm/versions/node/v24.18.1/bin:$PATH" pnpm install --frozen-lockfile --offline` | PASS; pnpm 11.19.0 |
| `PATH="$HOME/.nvm/versions/node/v24.18.1/bin:$PATH" pnpm --filter console test` | PASS; 31 tests |
| `PATH="$HOME/.nvm/versions/node/v24.18.1/bin:$PATH" pnpm --filter console build` | PASS |
| `python3` OCI-cell helper invoking `oci_cell_checks(GateRunner(ci=True))` | PASS; dry-run, BuildKit syntax check, evidence-boundary assertion |
| `docker buildx build --check --platform linux/amd64 --build-arg WORLDSTREAM_BASE_IMAGE=alpine@sha256:48b0309ca019d89d40f670aa1bc06e426dc0931948452e8491e3d65087abc07d packaging/oci` | PASS; BuildKit reported no warnings |
| `python3 scripts/gates.py minimal-ci --list-cells --format matrix` | PASS; Linux x86-64, Windows x64, OCI Linux/amd64 routes emitted |
| `python3 scripts/gates.py minimal-ci --ci --cell oci-linux-amd64` on this host | FAIL closed; route requires Linux, host is macOS arm64 |
| `python3 scripts/gates.py pre-push --ci --strict --offline` with pinned Node/pnpm | FAIL closed; 56 checks, 12 failures (missing optional `WORLDSTREAM_POSTGRES_URL` under strict CI mode plus unresolved release evidence contract rows) |
| `PATH="$HOME/.nvm/versions/node/v24.18.1/bin:$PATH" python3 scripts/gates.py release --offline --strict` | FAIL closed; 52 checks, 15 failures: manifest not release-ready, unresolved evidence/contracts, and missing release directory |
| `bash scripts/package-release.sh --target source --source-dir . --output <fresh-temp-dir>/out` | FAIL closed before output creation because manifest is specification-only |
| `bash scripts/package-oci.sh --binary-dir target/debug --ui-dir web/console/dist --sdk-dir sdk/python --examples-dir examples --licenses-dir licenses --base-image alpine@sha256:48b0309ca019d89d40f670aa1bc06e426dc0931948452e8491e3d65087abc07d --output <fresh-temp-dir>/out` | FAIL closed before output creation because manifest is specification-only |
| `bash -n scripts/package-release.sh scripts/package-oci.sh scripts/verify-release.sh` | PASS |
| Ruby YAML parse of `.github/workflows/compatibility-gates.yml`; `git diff --check`; Python compilation | PASS |

`actionlint`, PowerShell, `syft`, and native Windows tooling are unavailable on this host. A disposable Linux/amd64 Docker container was used only to confirm the container platform route; it is not product or release evidence.

## External blockers that remain fail-closed

- Native Linux x86-64 and Windows x64 package/runtime/ACL evidence requires their native hosted runners.
- OCI image build, non-root runtime, filesystem rejection, image digest, and packaged probes require a real Linux/amd64 evidence job and release-selected base image.
- The manifest owner must resolve pack/bundle/artifact/evidence digests and deliberately promote the manifest to release-ready.
- Checksums, Sigstore/OIDC signature, SPDX SBOM, and SLSA provenance require the release identity and artifact-producing job.
- Full transfer, backend-native restore, kill-point, privacy, filesystem, telemetry-backpressure, and one-hour soak evidence remain unresolved in the manifest; local tests do not promote those rows.

No Linear status was changed and no release claim was made from this arm64/macOS host.

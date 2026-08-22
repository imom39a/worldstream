# IMO-59/60/61 release toolchain and packaging continuation

Audit date: 2026-08-21

Owner: parent orchestrator, with Luna lanes for toolchain, source packaging,
and OCI/release evidence. Linear statuses were not changed.

## Scope and host boundary

This continuation covers the declared Node 24.18.1, pnpm 11.19.0, and Python
3.14.7 toolchains; the macOS source quickstart; source packaging and archive
verification; OCI context/build/runtime prerequisites; and safe local release
evidence generation.

The host is macOS 15.6.1 (Darwin 24.6.0), arm64, with the repository on APFS.
This host cannot provide native Linux x86-64, native Windows x64, or Linux/amd64
OCI release evidence. No such evidence is claimed below.

## Toolchain determination

The initial ambient shell was not pinned: Node was not 24.18.1, `pnpm` was
absent, and Python was 3.13.0. The existing installations were sufficient to
provision and use the declared pins without changing repository files.

Exact parent command:

```sh
NVM_DIR=${NVM_DIR:-$HOME/.nvm}
. "$NVM_DIR/nvm.sh"
nvm use --delete-prefix v24.18.1 >/dev/null
corepack enable
export PATH="$NVM_DIR/versions/node/v24.18.1/bin:$PATH"
corepack pnpm@11.19.0 --version
uv python install 3.14.7
uv run --python 3.14.7 --no-project python --version
```

Result:

```text
node=v24.18.1
corepack=0.35.0
pnpm-via-corepack=11.19.0
Installed Python 3.14.7
python-pin=Python 3.14.7
```

`uv` is `0.12.5` on this arm64 host. `uv python install` reported that an
existing unmanaged `~/.local/bin/python3.14` was not replaced, but
`uv run --python 3.14.7` selected the installed uv interpreter successfully.
After `corepack enable`, the pnpm shim was available at version 11.19.0.

## macOS source quickstart

Exact parent command:

```sh
NVM_DIR=${NVM_DIR:-$HOME/.nvm}
. "$NVM_DIR/nvm.sh"
nvm use --delete-prefix v24.18.1 >/dev/null
corepack enable
export PATH="$NVM_DIR/versions/node/v24.18.1/bin:$PATH"
uv run --python 3.14.7 --no-project bash scripts/macos-source-quickstart.sh
```

Result: exit `0`.

```text
macOS source quickstart: APFS, Python 3.14.7, Node/pnpm lockfile checks
52 passed in 27.90s
Done in 379ms using pnpm v11.19.0
Test Files 3 passed; Tests 23 passed
vite build completed
compatibility manifest verified (specification-only, release_ready=false)
macOS source quickstart checks passed; no signed or notarized binary was produced.
```

The run covered the locked Rust build, locked Python sync/tests, frozen pnpm
install, UI tests/build, and manifest verification. No script change was
needed: the pinned existing-tool path succeeds, and the script correctly
remains source-only.

## Source packaging and verification

Parent checks:

```sh
bash -n scripts/package-oci.sh scripts/package-release.sh \
  scripts/verify-release.sh scripts/macos-source-quickstart.sh
uv run --python 3.14.7 --no-project python tests/package_smoke.py
uv run --python 3.14.7 --no-project python tests/manifest_evidence_wave6.py
```

Results: shell syntax passed; package smoke passed; manifest evidence passed
with 4 tests. The package smoke release-valid inputs are synthetic fixtures,
not repository release artifacts. It verified deterministic tar/zip/source
archives, checksums, archive safety, OCI context inventory/verification, and
the modeled health/readiness/version probe.

Safe dry-runs all exited `0` (the temporary directory was created with
`TMP_DIR=$(mktemp -d /tmp/worldstream-release-continuation.XXXXXX)`):

```sh
scripts/package-release.sh --target source --source-dir . --output "$TMP/source" --dry-run
scripts/package-release.sh --target linux-x86_64 --output "$TMP/linux" --dry-run
scripts/package-release.sh --target windows-x64 --output "$TMP/windows" --dry-run
```

The source plan contained 249 deterministic files. Linux and Windows plans
contained 32 files each and explicitly reported their native binary inputs as
blocked. A real source archive attempt exited `1` with:

```text
package verification failed: release packaging requires manifest_kind=release
```

No archive or package report was emitted. This is the expected fail-closed
behavior for the checked-in specification manifest.

## OCI checks

Parent host probes:

```text
Docker server 29.4.0, os=linux, arch=arm64/aarch64
Docker buildx 0.33.0-desktop.1
Podman installed, but no usable machine/socket exists
```

The following safe checks exited `0` and reported planning-only boundaries:

```sh
scripts/package-oci.sh --output "$TMP/oci" --dry-run
scripts/package-oci.sh --output "$TMP/oci-dry" \
  --base-image alpine@sha256:0000000000000000000000000000000000000000000000000000000000000000 --dry-run
```

The dry-runs described an OCI `linux/amd64` context, non-root UID 65532,
read-only-root compatibility, `/var/lib/worldstream`, and the ext4/xfs
SQLite policy. They also reported the missing release binaries. The temporary
placeholder was used only to exercise argument parsing and was not recorded as
an image identity or release digest.

An actual context attempt exited `1` with the same `manifest_kind=release`
fail-closed error. Verifying `packaging/oci` directly also failed because the
checked-in directory is a template, not a generated context. A Docker build
probe against that template stopped before building because
`WORLDSTREAM_BASE_IMAGE` was unset. No OCI image, image digest, or runtime
probe was produced. Podman was not initialized or started.

## Pinned release gate

The parent ran the release gate with Node 24.18.1, pnpm 11.19.0, and uv Python
3.14.7 selected in `PATH`:

```sh
WORLDSTREAM_RELEASE_DIR="$TMP/missing" \
  scripts/gates.py release --offline --report "$TMP/report.json" \
  --handoff "$TMP/handoff.json"
```

Result: exit `1`, 52 checks, 17 failures. The pinned toolchain checks all
passed, as did Node lock resolution, Rust format/lint/build/tests, UI
lint/tests/build, local SQLite and operational evidence probes, and the
PostgreSQL contract mapping. The failures were the intentionally unresolved
manifest/release-evidence rows, contract rows mapped to those unresolved
rows, the missing release directory, and a transient Python SDK format/test
failure while other shared-worktree agents were active.

The Python SDK failures were independently rerun after the gate:

```sh
uv run --project sdk/python --locked --python 3.14.7 ruff format --check
uv run --project sdk/python --locked --python 3.14.7 pytest -q
uv run --project sdk/python --locked --python 3.14.7 pytest -q \
  sdk/python/tests/test_room_client.py::test_lost_action_reply_reuses_exact_identity
```

Results: 115 files already formatted; 58 tests passed; focused test passed.
No SDK or packaging change was made for the transient gate result.

## Changes and remaining blockers

The three Luna lanes added these evidence reports:

- `docs/agents/imo-release-toolchain-luna.md`
- `docs/agents/imo-release-packaging-luna.md`
- `docs/agents/imo-release-oci-luna.md`

This required continuation report is the parent orchestration record. No
packaging, script, workflow, focused packaging-test, compatibility manifest,
artifact, signature, SBOM, or provenance file was changed. No digests were
invented.

The remaining blockers are external release evidence: a release-ready
manifest with independently resolved fields; native Linux and Windows build,
ACL, filesystem, and runtime runs; Linux/amd64 OCI build/runtime evidence with
a release-selected pinned base image; full migration/transfer/restore,
kill-point, privacy, capability, lease, filesystem, and telemetry evidence;
and exact release checksums, Sigstore bundle, SPDX SBOM, and SLSA provenance.
The release gate remains correctly fail-closed until those inputs exist.

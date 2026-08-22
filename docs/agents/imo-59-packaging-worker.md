# IMO-59 packaging worker report

Date: 2026-08-21
Repository: `/Users/vinothshanmugam/code/agent-streamer`
Scope: host-independent packaging, OCI policy/runtime smoke, and macOS
source-quickstart checks only.

This lane did not update Linear, compatibility manifests, workflows, Rust
crates, transfer/backup/browser code, or another agent's report.

## Changes

- `scripts/package.py`
  - Embedded explicit backend/path policy in `metadata/profile.json`: startup-
    fixed `sqlite-bundled` default, direct-offline PostgreSQL administration,
    supported PostgreSQL connection modes, secret-source kinds, and explicit
    data/backup/export non-package boundaries.
  - Runtime probing now accepts `--storage-profile` and rejects `/version`
    responses whose selected engine profile differs from the requested profile.
  - Native/OCI payload validation rejects unsupported platform/distribution
    names inside otherwise-allowed payload trees, including ARM64, Windows
    container/service, MSI/MSIX, cloud, Kubernetes, and HA markers.
  - Native Windows input validation rejects reparse-point files in addition to
    symlinks, unsafe permissions, UNC paths, and secret-like names.
- `scripts/oci-runtime-smoke.sh`
  - Verifies the running container's actual `HostConfig.ReadonlyRootfs` flag
    and probes that a root-filesystem write is rejected before checking the
    explicit persistent volume.
- `scripts/macos-source-quickstart.sh`
  - Enforces `.node-version`, root `packageManager` pnpm version, and the
    selected uv Python version before the locked source build and tests.
  - Runs manifest verification through the pinned uv environment.
- `tests/package_smoke.py`
  - Covers backend/path metadata, unsupported payload-name rejection, and the
    selected-profile runtime probe contract.
- `tests/oci_runtime_smoke.py`
  - Covers read-only-root smoke requirements and provides an opt-in disposable
    real Docker `linux/amd64` run using a digest-pinned Alpine base. The
    disposable fixture is explicitly not release evidence.

## Verification

Passed:

```text
python3 tests/package_smoke.py                                      PASS
python3 tests/oci_runtime_smoke.py                                  PASS
python3 -m py_compile scripts/package.py tests/package_smoke.py tests/oci_runtime_smoke.py  PASS
bash -n scripts/package-release.sh scripts/package-oci.sh scripts/oci-runtime-smoke.sh scripts/macos-source-quickstart.sh  PASS
uv run --project sdk/python --locked ruff check scripts/package.py tests/package_smoke.py tests/oci_runtime_smoke.py  PASS
uv run --project sdk/python --locked ruff format --check scripts/package.py tests/package_smoke.py tests/oci_runtime_smoke.py  PASS
python3 scripts/verify-manifest.py                                    PASS
SOURCE_DATE_EPOCH=0 scripts/package-release.sh --target linux-x86_64 --dry-run  PASS (blocked inputs reported)
SOURCE_DATE_EPOCH=0 scripts/package-release.sh --target windows-x64 --dry-run  PASS (blocked inputs reported)
SOURCE_DATE_EPOCH=0 scripts/package-oci.sh --dry-run                  PASS (unpinned base/binaries reported)
```

The package smoke generated and independently verified deterministic synthetic
Linux `tar.gz` and Windows `zip` archives. It checked member layout, modes,
timestamps, checksums, matching compatibility manifest, SDK/UI identity, and
forbidden-path negatives. These are fixture mechanics, not native release
artifacts.

With Docker available, the parent later corrected the filesystem probe to read
Linux `/proc/self/mountinfo`. GNU `stat -f -c %T` reports the shared ext-family
magic as `ext2/ext3` even for ext4 and therefore could neither prove nor safely
reject the frozen ext4-only policy. The mountinfo probe observes the kernel's
actual filesystem name and still rejects a real ext2/ext3 mount.

Fresh parent result:

```text
WORLDSTREAM_RUN_OCI_DOCKER=1 python3 tests/oci_runtime_smoke.py             exit 0
  real digest-pinned linux/amd64 image build: PASS
  non-root/read-only-root invocation and negative SQLite policy checks: PASS
  explicit named-volume mount identity and writable persistence: PASS
  binary healthcheck: PASS
```

The disposable OCI build used:

```text
alpine:3.22.1@sha256:4bcff63911fcb4448bd4fdacec207030997caf25e9bea4045fa6c8c44de311d1
```

The integration result is not release evidence and is never promoted into the
compatibility manifest. The checked-in OCI template remains intentionally
unpinned and therefore returns `INCOMPLETE/pinned_base_image_required` in the
runtime smoke.

The current macOS quickstart attempt returned the expected fail-closed result
before building because `pnpm` is absent. The host also has Node 23.1.0 and
Python 3.13.0 while the repository pins Node 24.18.1 and Python 3.14.7. No
binary, signing, notarization, or release readiness claim was made.

## Acceptance boundary

| IMO-59 criterion | Truthful result from this lane |
| --- | --- |
| Deterministic Linux/Windows archive contents | Synthetic archive mechanics pass; native archive/runtime evidence still requires declared native runners and release-resolved inputs. |
| Backend profiles, health/readiness/version, PostgreSQL administration | Profile metadata and selected-profile probe are enforced; live native package probes and PostgreSQL administration remain external runtime evidence. |
| Checksums and matching manifest | Synthetic archives and OCI contexts verify exact checksum/manifest coverage; current repository is specification-only. |
| OCI non-root/read-only-root/volume/ephemeral rejection | Docker build, exact mount classification, writable named-volume persistence, binary healthcheck, and negative checks pass for the disposable digest-pinned fixture. No release image/signing claim is made. |
| Permissions, paths, secrets, data/backup/export boundaries | Archive modes, path safety, secret-like inputs, explicit non-package paths, and Windows reparse rejection are enforced. Native Windows DACL and native filesystem evidence remain external. |
| SDK/UI compatibility identity | Synthetic archive/context identity checks pass. |
| macOS source quickstart | Exact pin checks are implemented; this host is missing the pinned toolchain and produced no source-quickstart pass. |
| Forbidden artifacts | Payload-path validation rejects unsupported artifact markers; no ARM64, Windows container, MSI/MSIX, service, cloud, Kubernetes, or HA artifact was produced. |

## External blockers

- `compatibility.toml`/`.json` still declare `manifest_kind = "specification"`,
  `release_ready = false`, unresolved release fields, and empty release
  artifact digests. Real archives and a release context therefore fail closed.
- Native Linux x86-64 archive/runtime/filesystem evidence requires the declared
  native Linux runner. Native Windows x64 archive/runtime, DACL, reparse, and
  filesystem evidence requires the declared Windows runner and PowerShell.
- A release OCI image still requires release-selected WorldStream binaries, a
  generated release context, final image digest, and supply-chain evidence.
  The disposable linux/amd64 runtime fixture now establishes the OCI policy
  behavior but remains explicitly non-release evidence.
- Sigstore identity/bundle verification, SPDX SBOM generation, and SLSA
  provenance remain release-pipeline/signing infrastructure responsibilities;
  no signed artifact or release readiness was fabricated.
- A fresh macOS quickstart pass requires Node 24.18.1, pnpm 11.19.0, and
  Python 3.14.7.

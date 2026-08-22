# IMO-59/60 source packaging — Luna

Date: 2026-08-21

## Scope and host

Read `docs/release-packaging.md` and the current `scripts/package.py`,
`scripts/package-release.sh`, `scripts/package-oci.sh`,
`scripts/verify-release.sh`, `scripts/macos-source-quickstart.sh`, focused
packaging tests, and the compatibility-gates workflow.

Host evidence:

```text
uname -a: Darwin MacBookPro 24.6.0 Darwin Kernel Version 24.6.0: Mon Jul 14 11:30:29 PDT 2025; root:xnu-11417.140.69~1/RELEASE_ARM64_T6000 arm64
uname -m: arm64
cargo: cargo 1.97.1
node: v25.9.0
pnpm: missing
uv: 0.12.5
docker: 24.0.2
cosign: v3.1.3, darwin/arm64
```

The source quickstart confirmed APFS, then stopped before any build because
`pnpm` is missing. Python-only checks used the installed `PYENV_VERSION=3.13.0`
interpreter as a local tooling workaround; this is not pinned Python 3.14.7
quickstart evidence.

## Commands and results

Manifest parity:

```sh
PYENV_VERSION=3.13.0 python3 scripts/verify-manifest.py
# compatibility manifest verified (specification-only, release_ready=false)
# exit 0

PYENV_VERSION=3.13.0 python3 tests/manifest_evidence_wave6.py
# 4 tests, OK
# exit 0
```

Source packaging was attempted with a fixed epoch and an external temporary
output directory:

```sh
SOURCE_DATE_EPOCH=0 PYENV_VERSION=3.13.0 \
  scripts/package-release.sh --target source --source-dir . \
  --output /tmp/worldstream-packaging-luna.2quqNN/dist \
  --report /tmp/worldstream-packaging-luna.2quqNN/source.report.json
# package verification failed: release packaging requires manifest_kind=release
# exit 1; no source archive or report was created
```

This is the expected fail-closed result for the checked-in
`manifest_kind = "specification"`, `release_ready = false` manifest.

Safe planning checks:

```sh
SOURCE_DATE_EPOCH=0 PYENV_VERSION=3.13.0 \
  scripts/package-release.sh --target linux-x86_64 --dry-run
# exit 0; 32 deterministic files planned; native binaries reported blocked

SOURCE_DATE_EPOCH=0 PYENV_VERSION=3.13.0 \
  scripts/package-release.sh --target windows-x64 --dry-run
# exit 0; 32 deterministic files planned; .exe binaries reported blocked

SOURCE_DATE_EPOCH=0 PYENV_VERSION=3.13.0 scripts/package-oci.sh --dry-run
# exit 0; pinned --base-image and native binaries reported blocked
# read-only-root, /var/lib/worldstream, and ext4/xfs policy printed
```

Archive, wrapper, parity, traversal, permission, OCI-context, and structural
evidence mechanics:

```sh
PYENV_VERSION=3.13.0 python3 tests/package_smoke.py
# package archive smoke passed
# exit 0
```

The focused test verified deterministic fixture Linux tar, Windows zip, source
tar, OCI context inventories, wrapper reports, archive safety, and structural-
only evidence behavior. Its release-valid inputs are synthetic fixtures and
are not repository release artifacts.

macOS source lane:

```sh
scripts/macos-source-quickstart.sh
# macOS source quickstart is missing required commands: pnpm
# Install the pinned toolchain before retrying; no build was attempted.
# exit 1
```

Safe local release gate:

```sh
WORLDSTREAM_RELEASE_DIR=/tmp/worldstream-release-evidence-luna.1bBi7O/missing \
  PYENV_VERSION=3.13.0 python3 scripts/gates.py release --offline \
  --report /private/tmp/worldstream-release-gate-luna.XXXXXX.json \
  --handoff /private/tmp/worldstream-release-handoff-luna.XXXXXX.json
# blocker matrix: blocked
# 50 checks; 19 failures; 0 incomplete skips
# manifest-parity PASS; Rust format/lint/build/tests, cargo audit, secret scan,
# SDK/local evidence probes, and operator smoke PASS
```

Syntax and diff checks:

```sh
bash -n scripts/package-release.sh scripts/package-oci.sh \
  scripts/verify-release.sh scripts/macos-source-quickstart.sh
# exit 0
PYENV_VERSION=3.13.0 python3 - <<'PY'
import ast
from pathlib import Path
for path in (Path("scripts/package.py"), Path("scripts/manifest-evidence-wave6.py"), Path("scripts/verify-manifest.py")):
    ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    print(f"AST_OK {path}")
PY
# exit 0
git diff --check
# exit 0
```

## Remaining blockers

- The compatibility pair intentionally remains specification-only with
  unresolved release fields and empty release-artifact digests. This lane did
  not edit either manifest or derive any digest from local bytes.
- Native Linux x86-64 archive/runtime evidence requires a native Linux host;
  native Windows x64 archive, ACL, and runtime evidence requires Windows.
  This arm64 macOS run makes no such claims.
- OCI release evidence requires a Linux/amd64 builder/runtime and a release-
  selected pinned base-image digest. No image was built or runtime-tested.
- Release assembly still needs the exact `SHA256SUMS`, verified Sigstore
  bundle, SPDX SBOM, and SLSA provenance from their owning external jobs.
  None were created or signed here.
- The macOS source quickstart needs the pinned Node/pnpm/Python toolchain;
  the current host has Node 25.9.0, no pnpm, and no selected pyenv Python
  3.14.7. No source-build result is claimed.
- The release gate also reports unresolved migration, transfer, restore,
  snapshot, kill-point, privacy, capability, lease, filesystem, and telemetry
  release-evidence rows. Local fixture passes do not resolve those manifest
  rows.

No Linear status was changed. No packaging defect was confirmed, so no code
patch was made in the packaging/scripts/workflow paths.

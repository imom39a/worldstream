# IMO-59/60 toolchain and release-gate follow-up

Audit date: 2026-08-20

This parent verification covers the packaging and compatibility-gate changes
from the Luna release lane. It records executable local evidence without
turning the bootstrap compatibility manifest into a release approval.

## Local toolchain inventory

| Tool | Result |
|---|---|
| Node | `v23.1.0` on the parent host; the required `24.18.1` cell is unresolved — fail |
| pnpm | Not installed on the parent host; the required `11.19.0` cell is unresolved — fail |
| uv | Not installed on the parent host; the required `0.12.5` cell is unresolved — fail |
| Python | `3.13.0` on the parent host; the required `3.14.7` cell is unresolved — fail |
| rustc/cargo | `1.97.1` — pass |
| cargo-audit | `0.22.2`; advisory scan passes |
| gitleaks | `8.30.1`; repository scan passes |

## Parent checks

- `python3 tests/package_smoke.py`: pass; deterministic archive bytes,
  checksum metadata, secret-input rejection, symlink rejection, duplicate TAR
  rejection, and release-path traversal rejection all passed.
- `cargo test --workspace --locked`: pass; all workspace tests and doc tests
  pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass.
- `sdk/python/.venv/bin/pytest -q sdk/python/tests examples/heist`: pass;
  the parent run reports 24 tests.
- The available local UI toolchain (`node_modules/.bin/vitest`, `tsc`, and
  `vite`) passes 10 tests, type checking, and the production build. The
  pnpm/Node lock cells remain unresolved because the pinned host toolchain is
  unavailable.
- `scripts/gates.sh pre-push --offline`: exit 1 with 50 checks, 11 failures,
  and 6 incomplete skips. The failures are the pinned host tool cells and
  unresolved migration, transfer, restore, snapshot, combined kill/failure/
  fuzz/resource/power-loss, privacy, capability, lease, filesystem, and
  telemetry contract rows.
- `scripts/gates.sh release --offline`: exit 1 with 46 checks, 19 failures,
  and no incomplete skips. It fails closed on `release_ready=false`, the
  unresolved manifest and contract fields, unavailable release toolchain
  cells, and the missing `dist/release-manifest.json`.
- `scripts/package-release.sh --target linux-x86_64 --dry-run` and
  `scripts/package-oci.sh --dry-run`: planning passes only; no release artifact
  or OCI context was created.

## Correctness changes from the Luna lane

- Release-tier execution is always strict, so unavailable release scanners
  cannot remain incomplete skips.
- A terminal release manifest may use an empty unresolved-required-fields list;
  the bootstrap manifest still must remain `release_ready=false`.
- Release artifact and checksum paths are constrained to the release directory,
  reject traversal, backslashes, symlink escape, duplicate checksum entries,
  and unsafe archive members.
- TAR verification rejects duplicate member names and unsafe backslash paths,
  matching the ZIP safety policy.
- The parent pinned `time` to `0.3.47` and refreshed `Cargo.lock` after
  `cargo-audit` identified RustSec advisory RUSTSEC-2026-0009 in `0.3.44`;
  the current advisory scan is clean. The gate now invokes the current
  cargo-audit CLI without the obsolete `--locked` subcommand flag.

The parent also recorded the now-resolved PostgreSQL 17.11 direct/transaction-
pooler evidence row in `compatibility.toml`/`compatibility.json`. The manifest
still intentionally remains a specification with unresolved native restore,
transfer, failure/soak, privacy/security, and release-artifact evidence.

## Shannon packaging/release-verifier handoff

The packaging lane tightened only the scoped deterministic tooling; it did not
change the bootstrap compatibility pair or make a release claim. The verifier
now fails closed on:

- manifest schema/source/mirror/version identity, duplicate unresolved fields,
  exact release-artifact IDs, profile assignments, digest algorithms, and
  release-ready digest/status completeness;
- deterministic artifact filenames and profile/platform matching, explicit
  exclusion of ARM64, macOS binary, Windows-container, MSI/MSIX, service,
  Kubernetes/Helm, and cloud artifact names, plus rejection of unlisted files
  and symlinks in an evidence directory;
- per-artifact `sha256:` inventory digests, exact payload coverage in
  `SHA256SUMS`, and no checksum self-reference or supply-chain digest cycle;
- explicit release-manifest schema/source-version/compatibility-mirror digest,
  stronger Sigstore bundle, SPDX, and SLSA/in-toto document shapes, and SLSA
  subject-to-payload digest matching. Structural verification still returns
  exit `11` when cosign identity/issuer or cosign itself is unavailable;
- OCI template policy and generated metadata identity: pinned base image,
  Linux/amd64 target, UID/GID 65532, read-only-root/volume policy, and ext4/xfs
  SQLite allow-list. The generator still never fabricates an image, signature,
  SBOM, provenance document, Windows artifact, or release readiness.

Parent-verifiable evidence for this lane:

```text
python3 tests/package_smoke.py                         # pass
python3 -m py_compile scripts/package.py tests/package_smoke.py  # pass
bash -n scripts/package-release.sh scripts/verify-release.sh     # pass
scripts/package-release.sh --target linux-x86_64 --dry-run      # pass; blocked inputs reported
scripts/package-oci.sh --dry-run                                # pass; missing digest reported
git diff --check -- scripts/package.py tests/package_smoke.py    # pass
```

The positive smoke cases cover reproducible Linux/Windows/source archives,
runtime identity probes, and reproducible OCI contexts. Negative cases cover
weak Sigstore/SPDX/SLSA shapes, unsupported ARM64 artifact names, checksum
malformation, traversal, duplicate members, unpinned OCI inputs, and missing
release artifacts. The repository compatibility pair remains
`manifest_kind = "specification"`, `release_ready = false`; real release
verification remains impossible until the upstream evidence rows and artifact
digests are genuinely populated.

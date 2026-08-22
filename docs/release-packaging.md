# Release packaging

WorldStream ships four explicitly bounded distribution surfaces:

- native Linux x86-64 `tar.gz` archives;
- native Windows x64 `zip` archives;
- Linux/amd64 OCI contexts/images; and
- a macOS source-build quickstart only.

`scripts/package.py` is the deterministic implementation. The shell and
PowerShell wrappers only select the interpreter. Packaging reads the version
and manifest identity from `compatibility.toml`/`compatibility.json`, requires
their canonical parity, and writes a sorted `checksums.sha256` plus
`metadata/release.json`, immutable `metadata/profile.json`, and canonical
`metadata/build.json` into every archive. Build metadata binds the exact Git
commit, pinned material digests, toolchains, target/runner/compiler, and build
arguments. Profile metadata records the native target, storage profiles,
permissions, and the `/healthz`, `/readyz`, and `/version` probe contract.
`scripts/verify-release.sh` (or its PowerShell equivalent) verifies archive
traversal safety, exact permissions, checksums, version, profile metadata, and
manifest identity.

Release wrappers require the interpreter selected by
`WORLDSTREAM_RELEASE_PYTHON` (or the resolved `python3` on POSIX) to match the
exact `.python-version` pin. CI installs that interpreter through the pinned uv
toolchain before any payload is created; a missing or mismatched interpreter
fails before packaging.

The embedded compatibility manifest is a contract and platform inventory, not
the final byte inventory. Its release-artifact rows may be `status =
"detached"` with an empty `digest`, which is required when the row describes
the archive or another object that contains the manifest itself. The external
`release-manifest.json` (schema
`worldstream/release-artifact-manifest/v2`) owns the exact SHA-256 digest of
each finished archive, OCI image, checksum file, SPDX SBOM, and SLSA provenance
document. The Sigstore bundle is declared separately as path-only verification
material: its bytes authenticate the manifest through Cosign and cannot be
hashed inside the manifest they help verify. The signed provenance and
checksum verification remain mandatory; detaching an embedded digest never
relaxes finished-byte verification.

### Two-level release supply chain

Supply-chain evidence is intentionally produced in two levels. First,
`release-supply-chain.py` validates exactly thirteen typed source reports (all
release evidence except `checksums-signature-sbom-provenance`), copies those
source bytes into a closed subject inventory with the four payloads, and
generates/verifies `SHA256SUMS`, SPDX, and SLSA over that 17-subject set. SPDX
contains the locked Cargo, uv, and pnpm component graph and its source/build
relationships. Corepack installs pnpm only through the exact
`packageManager` value, which binds pnpm 11.19.0 to its SHA-512 registry
tarball integrity; the same URL and digest are retained in SPDX and SLSA.
SLSA contains the exact source commit, material digests,
toolchains, targets, runners, compiler/package arguments, and OCI base. Empty,
invented, or drifting graphs are rejected. The
inventory is keylessly signed and identity-verified before the typed
supply-chain producer report is emitted. That report binds the inventory,
inventory signature bundle, and all three sidecars by exact SHA-256 and byte
count, with observations for each verified check.

The collector then normalizes all fourteen reports. Assembly creates a
separate detached `release-manifest.json`, which is separately keylessly
signed. Final verification checks both signatures, the final artifact and
evidence digest maps, the thirteen source-report citations to the signed
inventory, and exactly the fourteenth supply-chain report. The supply-chain
report is not a subject of the inventory or the pre-sign sidecars, so no
attestation contains a digest of itself or of a signature over itself.

The native package command also checks the release identity before writing an
archive: the daemon's Cargo package, Python SDK `pyproject.toml`, and console
`package.json` must all use the manifest `contracts.product` version. Archive
verification accepts only the documented layout (daemon/operator binaries,
UI, SDK, examples, licenses, compatibility manifests, metadata, and
checksums); every member must be an ordinary regular file or a canonical
directory entry. Unlisted paths, symlinks, special members, traversal, and
unsafe permissions fail closed.

`licenses/THIRD-PARTY-NOTICES.json` and
`licenses/THIRD-PARTY-NOTICES.txt` are required package-identity materials,
not optional prose. The machine manifest covers every third-party
`Cargo.lock` package, every `pnpm-lock.yaml` package, and every APK installed
in the digest-pinned Alpine base. It binds Cargo checksums, pnpm SHA-512
integrities, the exact base-image package database digest, raw upstream
declared-license expressions, a pinned SPDX license-list revision, and every
notice-section SHA-256. Native archives, the source archive, and the OCI
context all carry those exact bytes; generation and verification reject a
missing component, lock drift, notice drift, or a substituted legal file.
Regenerate only after reviewing an intentional dependency or base-image
change:

```sh
uv run --python 3.14.7 --project sdk/python --locked python \
  scripts/generate-third-party-notices.py --write
```

Generation verifies registry crate checksums and npm tarball integrities,
uses license texts from an immutable SPDX license-list commit, and observes
the pinned image's APK database through Docker. Legacy declarations such as
`MIT/Apache-2.0` remain verbatim in the notice manifest and attribution; the
SPDX document uses the explicitly validated `MIT OR Apache-2.0` expression
and never emits the legacy spelling as SPDX syntax. The OCI SBOM models each
installed APK package separately beneath the aggregate base-image package.

The SDK and UI also carry `compatibility_identity.json`, derived canonically
from the embedded `compatibility.json`. The identity includes wire/config,
storage/core schema and hash identities, plus the complete `pack_executors`
rows: pack ID, explanatory version, every digest, exact revision digest, and
new-room/retained-room/status policy. Packaging rejects missing, malformed,
stale, or disagreeing SDK/UI identities before archive creation. Archive and
OCI-context verification repeats this comparison against the artifact's own
compatibility manifest, so generation-time validation cannot be trusted as a
substitute. The Python SDK exposes the loaded identity as
`worldstream_sdk.CLIENT_CONTRACT_IDENTITY`; the UI imports the single public
artifact with Vite's raw loader, parses it at runtime, exposes the exact
identity on `window.__WORLDSTREAM_CLIENT_CONTRACT_IDENTITY__`, and carries the
same artifact beside the built assets.

Source archives additionally verify that the UI source import points at the
public artifact and that the production entrypoint exposes that parsed value.
Native archives and OCI contexts verify that a production bundle contains the
exact canonical identity bytes and exposure marker, not merely a pack-digest
scan.

The verifier also checks the retained Agent Heist parity fixture imported by
the UI. Its `pack_id`, explanatory version, and `retained_executor.pack_digest`
must resolve to one exact manifest `pack_executors` row, and embedded UI bundle
`pack_digest` values must not be orphaned. This catches a fixture or rebuilt
bundle that lags the authoritative client identity.

The profile records expected probe statuses but does not claim that a live
service was contacted. Run the explicit runtime probe when a packaged daemon
is available; it checks liveness, the truthful 200/503 readiness contract, and
the complete manifest-backed `/version` summary, retained-pack rows, product
build identity, selected storage profile, and exact verified engine identity.
The reported `product_build.source_revision` must equal the packaged 40-hex
source commit exactly. Native and OCI runtime smoke also require the packaged
`worldstreamctl version` document to attest that same revision, preventing a
stale same-version control binary from passing:

```sh
scripts/package.py probe --base-url http://127.0.0.1:8080 --version 0.1.0 \
  --storage-profile sqlite-bundled
```

## Native archives

Build the binaries and UI on the target host, then package with a fixed
`SOURCE_DATE_EPOCH`. The static hosted job first selects `RUSTC`, the
target-specific `CC_<triple>` and `AR_<triple>`, and
`CARGO_TARGET_<TRIPLE>_LINKER`, builds with those exact paths, and captures the
runner and tool observations. For Linux, the workflow verifies the 53,733,924
byte Zig 0.15.2 Linux x86-64 archive against SHA-256
`02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239`
before using the committed `scripts/zig-musl-cc.sh` and
`scripts/zig-musl-ar.sh` drivers. Both drivers bind the bundled SQLite build to
Zig's `x86_64-linux-musl` compiler/sysroot rather than the hosted GNU libc
toolchain:

```sh
scripts/package.py capture-build-environment \
  --target linux-x86_64 \
  --output reports/native-linux-build-environment.json
SOURCE_DATE_EPOCH=0 scripts/package-release.sh \
  --target linux-x86_64 \
  --binary-dir target/x86_64-unknown-linux-musl/release \
  --ui-dir web/console/dist \
  --examples-dir examples \
  --licenses-dir licenses \
  --build-environment reports/native-linux-build-environment.json \
  --output dist
scripts/verify-release.sh dist/worldstream-<version>-linux-x86_64.tar.gz
```

For machine-readable handoff, the wrapper can persist the exact identity of
the archive it verified:

```sh
scripts/package.py capture-build-environment --target source \
  --output reports/source-build-environment.json
scripts/package-release.sh --target source --source-dir . \
  --build-environment reports/source-build-environment.json --output dist \
  --report dist/worldstream-<version>-source.report.json
scripts/verify-release.sh dist/worldstream-<version>-source.tar.gz \
  --report dist/worldstream-<version>-source.report.json
```

The report is deterministic JSON containing the portable archive filename, SHA-256, byte
size, successful archive verification, the exact observed upstream runner and
native compiler/archiver/final-linker identity, and distinct SHA-256 identities for
the authored `compatibility.toml` and canonical `compatibility.json` bytes.
The legacy `identity.manifest_sha256` remains an alias for the JSON digest;
new evidence adapters require both explicitly named fields. It is explicitly marked
`release_evidence: false`: it never creates signatures, an SBOM, provenance,
or a `release_ready` claim. `verify-release` checks that the report describes
the exact artifact bytes supplied on its command line.

The Windows command has the same shape with `--target windows-x64` and emits a
zip archive containing `.exe` binaries. Native packaging is required for the
respective target; cross-compilation is not evidence of the platform gate.
The pinned `windows-2025-vs2026` job enters the installed VS 2026 x64 developer
shell before resolving `cl.exe`, `lib.exe`, and `link.exe`; ordinary PowerShell
PATH discovery is not accepted as a compiler, librarian, or linker witness.
Archive verification rejects symlinks, special members, traversal, duplicate
members, group/world-writable members, and permissions other than `0755` for
directories/executables or `0644` for regular files. Windows ACL evidence
remains a native Windows release gate and is not fabricated by a Linux or
macOS run.

A source-only archive is available for the macOS/source profile and does not
claim a native binary or runtime release:

```sh
scripts/package.py capture-build-environment --target source \
  --output reports/source-build-environment.json
scripts/package-release.sh --target source --source-dir . \
  --build-environment reports/source-build-environment.json --output dist
scripts/verify-release.sh dist/worldstream-<version>-source.tar.gz
```

The native Linux and Windows archives contain:

```text
bin/worldstreamd[.exe]
bin/worldstreamctl[.exe]
ui/                        # built console, including hashed assets
sdk/python/                # locked Python SDK source distribution inputs
examples/heist/            # deterministic reference clients
licenses/
manifest/compatibility.toml
manifest/compatibility.json
metadata/release.json
metadata/profile.json
metadata/build.json
checksums.sha256
```

The source archive instead contains the deterministic, commit-bound
release-source subset beneath `source/`, plus its release/profile/build
metadata and checksum inventory. Every release-source input path must be
Git-tracked and the checkout must be clean; packaging adds only the generated
`source/.worldstream-source-revision` commit witness. Cache/dependency/output
directories (`.git`, the
listed tool caches, `node_modules`, `target`, `artifacts`, `coverage`, `dist`,
`package-extracted`, `package-input`, `release-inputs`, and `reports`) and
`tmp`—including its tracked research PDFs—are intentionally excluded by the
exact `SOURCE_ARCHIVE_EXCLUDED_DIRECTORY_NAMES` policy in `scripts/package.py`.
The archive does not claim to be a full checkout, native binaries, a built
console, or a native runtime.

The repository can emit deterministic release-candidate archives because the
portable compatibility contract is complete with `manifest_kind = "release"`
and `release_ready = true`. Those unsigned outputs are not a verified release.
Publication still fails closed until every hosted platform and behavior report
is bound into the detached subject inventory and its checksums, SBOM,
provenance, and keyless signature all verify. A planning-only dry-run is:

```sh
scripts/package-release.sh --target linux-x86_64 --dry-run
```

Dry-run output is planning information, not release evidence and never changes
the compatibility manifest.

## OCI

Generate a build context from a release-valid manifest:

```sh
scripts/package.py capture-build-environment --target oci-linux-amd64 \
  --output reports/oci-build-environment.json
scripts/package-oci.sh --output dist/oci \
  --base-image alpine@sha256:<64-lowercase-hex-digest> \
  --build-environment reports/oci-build-environment.json \
  --report dist/worldstream-<version>-oci.report.json
docker buildx build --platform linux/amd64 \
  --load --provenance=false \
  --output type=oci,dest=dist/worldstream-<version>-oci-linux-amd64.oci.tar,compression=gzip,force-compression=true \
  --tag worldstream-release:<version> \
  --build-arg VERSION=<version> \
  --build-arg MANIFEST_SHA256=<sha256> \
  --build-arg SOURCE_REVISION=<40-hex-commit> \
  --build-arg BUILD_IDENTITY_SHA256=<sha256> \
  --build-arg BUILD_ENVIRONMENT_BASE64=<canonical-base64-from-oci-metadata> \
  --build-arg SOURCE_DATE_EPOCH=0 \
  --build-arg WORLDSTREAM_BASE_IMAGE=<image@sha256:digest> \
  -f dist/oci/Dockerfile dist/oci
```

The workflow supplies Buildx 0.36.1 and creates its `docker-container` builder
with the exact BuildKit image recorded in the workflow; substituting the
default Docker builder, an unpinned Buildx, or another BuildKit image fails the
producer contract.

`capture-build-environment` is release-only and fails outside the canonical
main-branch GitHub Actions workflow. It records the provider, runner OS and
architecture, hosted image and image version, plus the exact selected rustc,
bundled-SQLite C compiler and archiver, and final linker paths, version output,
payload target, and independently probed host/tool target or archive format.
Source archives record only their
upstream runner because they perform no native compilation. Packaging rejects
a missing, local, partial, or target-drifted capture. `metadata/build.json`,
the package/context report, the OCI metadata and label, and the SLSA payload
row carry the same canonical observation; release aggregation compares them
byte-for-byte.

The image is designed for `--user 65532:65532 --read-only`, declares the
explicit `/var/lib/worldstream` volume, and has a binary `worldstreamctl`
healthcheck that connects to the configured daemon and requires the exact
`GET /healthz -> 200 {"status":"ok"}` contract; a standalone CLI process with
valid configuration but no daemon must fail. Startup still requires the
authority bootstrap material through an owner-readable secret file; the OCI
runtime smoke mounts that file from a separate read-only secret volume and
never places it in the image, environment, command line, report, or logs. With
bundled SQLite selected, its entrypoint rejects every data
directory other than `/var/lib/worldstream` and every filesystem other than
ext4 or xfs. The entrypoint reads Linux mountinfo and also requires a nonzero
block-device identity with a `/dev/*` mount source, so the ambiguous GNU `stat`
label `ext2/ext3` cannot be mistaken for proof of ext4 and a network mount
cannot pass merely by occupying the configured path. Overlay, tmpfs, NFS,
SMB/CIFS, FUSE, virtual/non-block mount identities, and missing or non-mounted
data directories are rejected before the daemon starts. The release runtime
smoke additionally requires Docker's `local` volume driver and `local` scope
with no driver options, actively proves that an NFS-configured local-driver
volume is rejected, and records the driver, scope, filesystem, device, and
mount source in the typed report. PostgreSQL remains an opt-in profile; none of
these checks promotes a network-backed SQLite layout to supported.

The exact base image is checked in at `packaging/oci/base-image.txt` and is
required by the context generator; a different or unpinned `FROM` is rejected.
The OCI runtime smoke also hashes `/lib/apk/db/installed` inside the completed
image and requires equality with the notice manifest, preventing a stale
same-tag notice inventory or an unreported base package change from passing.
The generated context records the exact base reference, source commit, build
identity digest, source epoch, runtime UID, volume, filesystem policy, and
probe contract. No
Windows container, ARM64 image, MSI/MSIX, Windows Service, cloud, Kubernetes,
or HA artifact is produced.

The generator verifies the completed context before returning. The optional
report contains the portable context name and a canonical path/size/SHA-256 inventory for every regular
context file and is explicitly marked `release_evidence: false`; it does not
claim that Docker built an image or that an OCI runtime was exercised. The
same context can be verified later, including its inventory, with:

```sh
scripts/verify-release.sh dist/oci \
  --report dist/worldstream-<version>-oci.verified.json
```

The verifier rejects symlinks and special files, traversal, unsafe
permissions/timestamps, manifest drift, non-canonical embedded metadata,
checksum coverage errors, and unpinned base-image metadata. The generated
context inventory fingerprints Dockerfile, entrypoint, runtime metadata,
packaged payload, and sidecars.

## Evidence bundle verification

When a release evidence directory exists, the same verifier accepts the
directory instead of an archive:

```sh
scripts/verify-release.sh dist
```

It requires `release-manifest.json`, `SHA256SUMS`, a Sigstore bundle,
`sbom.spdx.json`, and `provenance.json`; validates declared paths, exact SHA-256
coverage, the exact non-empty SPDX component/relationship and SLSA
source/material/build graphs, the GitHub Actions builder/runner identity, and
Sigstore document shape, and then runs `cosign verify-blob`
over `release-manifest.json` with `COSIGN_CERTIFICATE_IDENTITY` and
`COSIGN_CERTIFICATE_OIDC_ISSUER`. The signed manifest hashes every payload and
supply-chain sidecar except the Sigstore bundle that proves its signature; this
avoids self-reference while authenticating the complete subject inventory.
Missing cosign, identity, issuer, artifacts, or live evidence is a hard failure.
The verifier joins the external artifact digests to the embedded artifact IDs
and only compares to an embedded digest when that row is explicitly resolved.
A detached row must have no embedded digest. The verifier never creates a
signature, SBOM, provenance document, or release manifest.

For a document-only review, `--structural-only` stops before cryptographic
verification and returns exit status `11` to mark the result incomplete:

```sh
scripts/verify-release.sh dist --structural-only
```

The evidence verifier requires HTTP-independent release documents plus
cryptographic Sigstore verification. It never turns a document-shape check
into a signing claim.

## macOS

macOS is source-only. `scripts/macos-source-quickstart.sh` verifies macOS 15+
and APFS, checks that `cargo`, `node`, `pnpm`, `uv`, and Python are available
before starting, then runs the locked source build, SDK tests, frozen UI
install, UI tests/build, and manifest verification. Finally it starts the
source-built daemon on loopback with the explicit `sqlite-bundled` profile and
an ephemeral owner-only data directory/secret, checks `/healthz`, `/readyz`,
and the manifest-backed `/version`, and shuts the daemon down. The quickstart
deletes its temporary runtime state, does not create a macOS binary archive,
and makes no signing or notarization claim.

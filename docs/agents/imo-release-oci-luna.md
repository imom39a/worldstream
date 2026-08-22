# IMO-60/61 Luna OCI/release evidence handoff

Audit date: 2026-08-21

## Result

The OCI packaging and verification implementation is structurally sound on the
available local seams. No packaging defect was proven, so no packaging,
wrapper, or focused-test code was changed. The only added file is this
evidence handoff. Linear statuses were not changed.

This host is macOS 15.6.1 on arm64. Results below are explicitly separated:
local context/shape evidence is not Linux/amd64 image or runtime evidence.

## Manifest and local context generation

The checked-in compatibility pair remains specification-only:

```text
manifest_kind = specification
release_ready = false
unresolved_required_fields = 9
OCI artifact digest = unresolved
```

Command:

```sh
PYENV_VERSION=3.13.0 python3 scripts/verify-manifest.py
```

Result: exit `0`; `compatibility manifest verified (specification-only,
release_ready=false)`.

Safe OCI planning command:

```sh
PYENV_VERSION=3.13.0 scripts/package-oci.sh --dry-run \
  --output /tmp/worldstream-oci-dryrun \
  --base-image alpine:latest
```

Result: exit `0`; planned `OCI linux/amd64` context and reported the two
missing native binaries under `target/release`. No context was written.
Dry-run is planning/shape output only.

Real local generation was fail-closed:

```sh
out="$(mktemp -d /tmp/worldstream-oci-blocked.XXXXXX)"
PYENV_VERSION=3.13.0 scripts/package-oci.sh \
  --output "$out/context" --base-image alpine:latest \
  --report "$out/report.json"
```

Result: exit `1`, `package verification failed: release packaging requires
manifest_kind=release`; both context and report were absent. No base-image,
artifact, signature, SBOM, or provenance digest was invented.

Verifying the checked-in OCI template directly also failed closed because it
is not a generated context:

```sh
PYENV_VERSION=3.13.0 scripts/verify-release.sh packaging/oci
```

Result: exit `1`; required generated payload, manifest, metadata, and checksum
members were missing.

## Static and focused packaging checks

Commands:

```sh
PYENV_VERSION=3.13.0 python3 - <<'PY'
from pathlib import Path
for name in ("scripts/package.py", "tests/package_smoke.py", "scripts/verify-manifest.py"):
    compile(Path(name).read_text(encoding="utf-8"), name, "exec")
    print(f"compile ok: {name}")
PY

for f in scripts/package-oci.sh scripts/package-release.sh \
  scripts/verify-release.sh scripts/macos-source-quickstart.sh; do
  bash -n "$f"
done
sh -n packaging/oci/entrypoint.sh
```

Result: all Python compile and shell syntax checks passed. `shellcheck` and
`shfmt` are not installed, so those checks were not claimed.

Focused test:

```sh
PYENV_VERSION=3.13.0 tests/package_smoke.py
```

Result: exit `0`, `package archive smoke passed`. The test covers reproducible
native/source archives, OCI context generation and verification on a
release-valid fixture, wrapper reports, traversal/checksum/metadata rejection,
and the modeled runtime probe (`health=200`, `ready=503`, version `0.1.0`).
It is fixture/shape evidence only; it is not a built OCI image or Linux runtime
run.

## Host/build/runtime availability

Host probes:

```text
Darwin 24.6.0 ... RELEASE_ARM64_T6000 arm64
macOS 15.6.1 (24G90)
filesystem=apfs
```

Native source checks:

```sh
cargo check --workspace --all-targets
cargo test --workspace --all-targets
```

Result: both exit `0`. These are native arm64 Rust checks and do not certify
Linux/amd64 packaging or runtime behavior.

Docker/Podman:

```text
Docker 24.0.2; Docker Desktop server reachable; buildx 0.33.0 available
Podman 5.2.5; no Podman machine/socket available
```

The only Docker build probe used the checked-in template without a generated
context or base argument:

```sh
docker build --platform linux/amd64 -f packaging/oci/Dockerfile packaging/oci
```

Result: exit `1` before an image build, because
`WORLDSTREAM_BASE_IMAGE` was blank. No OCI image/runtime evidence is claimed.
Podman reported that its default machine/socket was unavailable; no machine
was initialized or started.

The source quickstart was attempted with the available Python pin selection:

```sh
PYENV_VERSION=3.13.0 scripts/macos-source-quickstart.sh
```

Result: exit `1` before build; `pnpm` is missing from PATH and the script
reported `no build was attempted`. This does not negate the separate pinned
toolchain handoff, but this lane does not claim a fresh quickstart run.

## Remaining external blockers

- A reviewed release manifest with `release_ready=true` and all unresolved
  release fields resolved is required before real archive/OCI generation.
- Linux/amd64 release evidence still needs a pinned base image, generated
  context, built image, image digest, and exercised Linux/amd64 container
  probes. The arm64 macOS context/shape checks above are not substitutes.
- OCI runtime evidence must exercise the declared non-root/read-only-root and
  persistent `/var/lib/worldstream` ext4/xfs policy.
- Sigstore verification needs a real release evidence bundle plus certificate
  identity/issuer; SPDX SBOM and SLSA provenance must come from the external
  release lane. `syft` and `slsa-verifier` are absent locally; `cosign` is
  installed but no release bundle or identity claim exists here.
- Native Windows x64 evidence remains external and is not inferred from this
  host.
- The current shell lacks `pnpm`; the macOS source quickstart remains
  unverified in this invocation.

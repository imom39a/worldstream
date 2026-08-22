# IMO-59 cross-platform evidence Luna lane

This lane records what can actually be proven from the macOS arm64 host and
implements the archive/release-inventory boundary in the scoped packaging
tools. It does not edit `compatibility.toml`, `compatibility.json`, workflows,
gates, or release evidence rows.

## Contract boundary

The compatibility manifest defines native Linux x86-64 and native Windows x64
release profiles. It separately defines an OCI Linux/amd64 profile. Therefore:

- a Docker `linux/amd64` image built and run on the arm64 Docker host is genuine
  OCI Linux/amd64 runtime evidence when the image architecture, pinned base
  digest, build output, and runtime result all agree;
- cross-compilation of a Rust or other Linux binary is not native Linux release
  evidence;
- a Windows cross-compiled executable, a Linux container, or a macOS run is not
  native Windows runtime/ACL evidence;
- Windows evidence remains `INCOMPLETE` until a native Windows runner exercises
  the required process, filesystem, ACL, and runtime checks.
- Embedded `release_artifacts` rows may be `status="detached"` with
  `digest=""` and `digest_location="release-manifest.json"`. This prevents an
  archive from embedding a digest of bytes that include that same manifest.
  The external release manifest, checksums, provenance, and Sigstore
  verification still bind the finished bytes exactly.

## Reproduction

Run:

```sh
bash scripts/cross-platform-evidence.sh
```

The script writes the exact commands and outputs to
`/tmp/luna-cross-platform-report.txt`. It probes Docker Buildx, pulls a pinned
`alpine:3.22.1@sha256:4bcff63911fcb4448bd4fdacec207030997caf25e9bea4045fa6c8c44de311d1`
image for `linux/amd64`, builds a disposable amd64 image with
`docker buildx build --platform linux/amd64 --load`, inspects its architecture
and image identity, and runs it. It also exercises the existing package dry
runs and static/OCI boundary checks. The disposable probe is not a WorldStream
release artifact and is always marked `release_evidence=false`.

Use `--no-docker` for the deterministic manifest/package boundary checks only.
That mode deliberately exits `13` (`INCOMPLETE`) because it cannot establish a
runtime claim.

## Detached release-inventory implementation

The scoped packaging verifier now accepts the parent-approved release shape:
each embedded `release_artifacts` row uses `status="detached"`, an empty
`digest`, and `digest_location="release-manifest.json"`. The detached external
inventory uses schema `worldstream/release-artifact-manifest/v2`. Archive
creation and archive verification still require canonical embedded manifest
bytes, exact metadata, complete checksums, safe members, and deterministic
permissions. Release-directory verification independently hashes every
finished artifact and compares only to its external `artifact_digests` map;
it does not compare a finished archive to a digest embedded inside that
archive.

Local verification on this host:

```text
python3 tests/package_smoke.py                         PASS
python3 tests/cross_platform_evidence.py              PASS
ruff check package/evidence Python files               PASS
ruff format --check package/evidence Python files      PASS
python3 -m py_compile package/evidence Python files    PASS
bash -n package/evidence shell scripts                 PASS
```

The package smoke deliberately includes a negative external-digest mismatch
case and still ends with `package archive smoke passed`. Its structural
release-bundle check reports the expected incomplete signature status because
cosign identity/issuer inputs are not available locally.

## Evidence recorded on 2026-08-21

The host is macOS arm64. Docker Desktop exposes an arm64 Linux daemon and a
BuildKit builder advertising `linux/amd64`. The probe therefore establishes
Docker-emulated Linux/amd64 container build and runtime evidence, including
the exact base image digest and the local image ID recorded in the report.

The captured tool/image identities were Docker client `24.0.2` (darwin/arm64),
Docker Desktop engine `29.4.0` (linux/arm64), Buildx
`v0.33.0-desktop.1` at commit `7f91f038ac14cbf5c4b2a6b76470860814424da`,
BuildKit `v0.29.0`, and Alpine `3.22.1` at
`sha256:4bcff63911fcb4448bd4fdacec207030997caf25e9bea4045fa6c8c44de311d1`.
The probe image ID was
`sha256:56635825ac34126e50309b341e456ba6cafce2d96447184820c45a8b41849f67`.
Its runtime printed `Linux x86_64` and the embedded proof marker, and the
report records the complete command/output transcript.

The same host cannot establish native Linux x86-64 release evidence or native
Windows x64 evidence. The Windows container platform is not a substitute for a
native Windows runner and is fail-closed as incomplete here. No claim is made
for Windows ACLs, Windows services, MSI/MSIX, power-loss behavior, or a signed
release artifact. The existing OCI runtime script remains incomplete when run
against the checked-in context because its base-image digest is intentionally a
release-time input.

The report is diagnostic handoff evidence only. It does not resolve detached
release-manifest digests, create signatures/SBOM/provenance, or set
`release_ready`.

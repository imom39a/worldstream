# Automated compatibility gates

The checked-in `compatibility.toml` is the authored source for the gate
framework. `compatibility.json` is regenerated with `cargo xtask compat generate`
and must remain a byte-for-byte deterministic mirror. Gate cells reference the
manifest platform IDs; an unresolved manifest field is evidence of incomplete
implementation, never a passing result.

Run the local tiers with the standard-library runner:

```sh
scripts/gates.sh fast
scripts/gates.sh pre-push --strict --offline
cargo run --locked -p xtask -- gates fast
```

PowerShell uses `scripts/gates.ps1` with the same arguments. `fast` is intended
for the local hook and checks manifest/source drift, formatting, focused Rust
lint and tests (including goldens), lock resolution, and tracked-file secret
patterns. `pre-push` adds the full Rust, Python SDK, UI, operator smoke, and
bundled-SQLite checks; it has a 1,200-second hard deadline in addition to its
360-second warm and 900-second cold targets. `--strict` turns incomplete optional checks into
failures; without it, a missing optional dependency is printed as
`SKIP_INCOMPLETE`.

`minimal-ci --cell <id> --ci` is the native matrix gate. The required Linux and
Windows IDs and their runner/shell routes are read from the manifest platform
IDs; an unknown platform or a non-native host fails matrix generation/execution.
Native cells require PostgreSQL only when their declared storage profiles include
`postgres-primary`. Hosted cells put the PostgreSQL connection string in an
owner-only regular file and expose only its path through
`WORLDSTREAM_POSTGRES_DSN_FILE`; the gate removes any password from the `psql`
argument and scopes it to that child process. `WORLDSTREAM_POSTGRES_URL`
remains a legacy, explicitly configured developer-only input. A local omission
is visible incomplete; CI does not allow it to become a silent pass.

The release tier is intentionally fail-closed. `release_ready = true` means
only that the embedded compatibility contract/build identity is complete; it
is not a claim that archive or evidence bytes have those identities. Signed
release artifact rows use `status = "detached"`, `digest = ""`, and
`digest_location = "release-manifest.json"`. The Sigstore row instead uses
`status = "verification_material"`, no digest algorithm or digest location,
and a path-only `verification_material_location` pointer into the detached
manifest. Release-gated evidence rows use the analogous `artifact_digest` and
`artifact_digest_location` fields. The
release directory (default `dist/`) must contain a detached v2
`release-manifest.json` with exact artifact and evidence path/digest maps. The
gate recomputes every file hash, rejects missing/extra/duplicate identities,
and rejects an inventory that attempts to hash itself.

The detached inventory's payload and evidence subjects must be covered exactly
by `SHA256SUMS`, the SPDX JSON SBOM's SHA-256 file entries, and the SLSA
provenance subjects. The detached manifest also records the exact hashes of
`SHA256SUMS`, the SBOM, and provenance. It excludes only the Sigstore bundle
from signed subjects and records that bundle as verification material, avoiding
the signing self-reference. The Sigstore bundle is verified cryptographically
over the detached `release-manifest.json` itself. Structural document shape
never counts as a signature, SBOM, or provenance identity; the release gate
requires `cosign` and its certificate identity/issuer inputs. Set
`WORLDSTREAM_RELEASE_DIR` for another directory. The runner does not populate
the embedded compatibility manifest or invent detached evidence.

The release assembly interface is `scripts/release-evidence-assemble.sh`; its
input contract derives the four deterministic payload filenames and the 14
unique `release_gate = true` evidence IDs from the compatibility pair. Each
report must be a regular `<evidence-id>.json` file with the same embedded ID,
`release_gate = true`, and a passed status. Missing, extra, symlinked,
non-regular, failed, or mismatched inputs stop assembly with the precise list;
the workflow does not create placeholder reports. The reviewed
`scripts/release-supply-chain.sh` generator emits SPDX-2.3 and in-toto/SLSA v1
documents for the 17 pre-sign subjects. `SHA256SUMS` is written over the four
payloads plus all 13 non-supply-chain reports; `release-manifest.json` records
the sidecar and evidence digests while keeping Sigstore path-only.

The manual GitHub release job builds native Linux and Windows archives, builds
and runtime-tests the Linux/amd64 OCI image, packages the source archive, and
uploads those outputs plus platform evidence. Required downstream jobs consume
the fresh Linux archive and package report: one runs the exact 36-cell SIGKILL
matrix and 3600-second daemon workload, while packaged acceptance runs Counter
and absent-Broker Heist across SQLite and pinned PostgreSQL. The separately
certified reference job then runs the archive's Python SDK against the packaged
SQLite daemon for the frozen 10,000-Room, 1,000-WebSocket, 1,800-second target
attempt. It binds—but never relabels as same-host—the independently produced
acceptance, soak, and kill reports. A source-bound production-Core fixture sets
up one Room with exactly 100,000 Transitions and a two-Transition snapshot tail;
only fresh packaged-daemon restart and verified Projection recovery are timed.
The unsigned aggregation job downloads the resulting typed producer artifacts
by exact path and generates the reviewed checksums/SBOM/provenance. A
checkout-free protected signer uses OIDC only to sign that closed inventory; a
no-OIDC finalizer assembles the detached manifest, a second checkout-free
protected signer signs only that manifest, and a no-OIDC verifier runs
`verify-release`. If
any producer or byte binding is absent, its required job or collector stops;
package reports and generic exit codes never become release evidence. The macOS
job remains source-only quickstart evidence on both Apple Silicon and Intel.

Reference measurements run only on a self-hosted runner carrying the exact
`ubuntu-24.04-x86_64-ext4-4vcpu-8gib-local-ssd` certification label. The typed
reference reports independently require observed Ubuntu 24.04/x86-64, exactly
four logical CPUs, exactly 8 GiB of effective memory, ext4 mount identity, and
a locally observed non-rotational SSD/NVMe data device. A generic GitHub-hosted
Ubuntu label or a missing certified runner leaves the manual release blocked;
the workflow does not infer those properties from its runner label.

Hosted fast/native/release gates install byte-pinned `gitleaks 8.29.1` and
`cargo-audit 0.22.2`, prime the locked Rust/Python/pnpm dependency stores, and
then run the gate offline. The Python gate invokes a sorted explicit
`tests/*.py` inventory because these repository tests intentionally do not use
pytest's default `test_*.py` filename pattern.

The kill-point probe uses Linux process semantics (`SIGKILL`) and is therefore
`SKIP_INCOMPLETE` on Darwin in local/pre-push tiers. The native Linux/release
cell is required to execute and pass it; a Darwin skip is never release
evidence.

The `provider-smoke` tier is credential-free and optional. It rejects provider
credential environment variables, runs only the local operator smoke when
available, and records dated provider verification as incomplete rather than
claiming a provider SLA, HA, region, or durability guarantee.

The runner audits every release-gated evidence row in addition to the grouped
contract diagnostics below. The following release-contract tiers are tracked
explicitly and remain incomplete until their evidence rows are resolved: migrations,
SQLite/PostgreSQL transfer, backend-native snapshot/restore and semantic
verification, kill-point/failure and soak checks, privacy, capability and
lease fencing, filesystem policy, and telemetry backpressure. This makes the
absence of an implementation or external harness visible in local output and
blocking in CI/release mode.

Set `WORLDSTREAM_GATE_REPORT=path/to/report.json` to persist the deterministic
outcome list together with `summary`, `blocking_failures`, and
`incomplete_skips`. The report is diagnostic gate output only; it does not
populate compatibility fields or turn incomplete evidence into a release.

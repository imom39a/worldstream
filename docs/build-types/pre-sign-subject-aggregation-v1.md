# WorldStream pre-sign subject aggregation build type v1

This document defines version 1 of the WorldStream SLSA build type. Producers
identify this definition with the immutable Git blob URL for the commit that
first published this file. Incompatible semantics require a new definition and
Type URI.

## Build boundary

The build is the `release-evidence` job's pre-sign aggregation phase. It begins
with four already-produced payloads and thirteen already-produced typed source
reports. The phase validates their embedded identities, copies the exact input
bytes into the closed release layout, and emits an in-toto Statement whose
seventeen subjects are those byte-for-byte copies.

Inventory signing happens after provenance generation. A signature, signature
bundle, final detached manifest, checksums document, SBOM, and the provenance
statement itself are neither inputs nor subjects of this build.

This build type does not claim that its runner compiled native payloads or made
the upstream measurements. Payload result rows record the validated embedded
source, material, toolchain, target, runner-route, compiler, package, UI, and
OCI relationships. Evidence result rows record the validated producer,
platform, checks, source, and configured runner route. `runDetails` describes
only the runner that validates and aggregates the final inputs.

## Parameters and dependencies

`buildDefinition.externalParameters` contains exactly:

- `trigger`: `workflow_dispatch`, `refs/heads/main`, and `release=true`;
- `source`: the canonical WorldStream repository and `refs/heads/main`;
- `manifest_inputs`: the reviewed TOML and JSON compatibility inputs;
- `payload_inputs`: four artifact-ID and logical-file-URI rows; and
- `evidence_inputs`: thirteen source-ID and logical-file-URI rows.

`buildDefinition.internalParameters` is exactly `{}`.

`buildDefinition.resolvedDependencies` contains the exact source commit, every
pinned source material, each commit-pinned GitHub Action, the digest-pinned OCI
base, BuildKit, and Dockerfile-frontend images, and the seventeen aggregation
inputs. Each descriptor uses the applicable `gitCommit` or `sha256` digest.

Every statement has a bijection between its subjects and producer result rows
by exact output path and SHA-256. The same payload and evidence bytes appear as
resolved dependencies at their logical input URIs because this phase validates
and copies upstream outputs.

## Run details and results

`runDetails.builder.id` is the canonical main-branch workflow ref and
`runDetails.metadata.invocationId` is the GitHub Actions run attempt. Timing is
omitted because the generator has no trustworthy whole-job start and finish
instants.

`runDetails.byproducts` contains exactly two canonical Resource Descriptors:
the observed aggregation-runner identity and
`worldstream-release-aggregation-v1.json`. The aggregation result records the
component-graph digest, four payload result rows, thirteen evidence result
rows, the source revision, reproducible epoch, and common toolchain identity.
Configured upstream routes are never represented as observed runner facts.

## Initiation

An authorized operator dispatches **Compatibility gates** on
`refs/heads/main` with `release=true`. The build begins only after all four
payloads and thirteen typed reports exist. Local runs can exercise the
structure using explicit local-test invocation data, but verifiers reject them
as release provenance because they do not have the canonical GitHub builder,
invocation, trigger, ref, input, and runner identity.

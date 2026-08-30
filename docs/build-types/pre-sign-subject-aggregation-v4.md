# WorldStream pre-sign subject aggregation build type v4

This document defines version 4 of the WorldStream SLSA build type. Its Type
URI is the content-addressed URN
`urn:worldstream:build-type:sha256:<definition-sha256>`, where the final segment
is the lowercase SHA-256 of the exact bytes of this file. Any incompatible
semantic or field change therefore produces a different Type URI and requires
a new definition and example path. The checked-in definition is also a pinned
source material, so an offline verifier can resolve and hash it without network
access.

A complete illustrative statement is published next to this definition as
`pre-sign-subject-aggregation-v4.example.json`. The example is documentation,
not release evidence or a resolved dependency.

## Build boundary

The build is the `release-evidence` job's pre-sign aggregation phase. It starts
with four independently built Runtime payloads, twelve portable Starter
subjects, and seventeen independently produced typed source reports. It
validates their identities, copies the exact input bytes into the closed release
layout, and emits an in-toto Statement whose thirty-three subjects are those
byte-for-byte copies.

The twelve portable subjects are the A202 adapter, deterministic agents,
documentation, examples, licenses, exact official Negotiate bundle, Negotiate
evidence verifier, Pack toolchain, Participant Console, release metadata,
Studio, and TypeScript Pack SDK. Their deterministic filenames and SHA-256
digests are part of this graph. The Negotiate `.wspack` additionally retains
its BLAKE3 Bundle and Revision identities under the compatibility contract.

Signing occurs later. The pre-sign inventory signature, final detached
manifest, final manifest signature, checksums, SBOM, and provenance statement
are neither inputs nor subjects of this build. Post-sign Starter and outside-
adopter qualification is bound by the separate release qualification manifest
and is not recursively inserted into this pre-sign graph.

## Closed statement shape

The statement has exactly `_type`, `subject`, `predicateType`, and `predicate`.
`_type` is `https://in-toto.io/Statement/v1`; `predicateType` is
`https://slsa.dev/provenance/v1`. `subject` contains exactly thirty-three unique
rows sorted by name. Each row has exactly `name` and
`digest: {"sha256": HEX}` and corresponds bijectively to one Runtime payload,
portable subject, or evidence producer row.

`predicate` has exactly `buildDefinition` and `runDetails`.
`buildDefinition` has exactly `buildType`, `externalParameters`,
`internalParameters`, and `resolvedDependencies`. Unknown, missing, duplicate,
non-canonical, or incorrectly ordered values are rejected.

## Parameters and dependencies

`externalParameters` has exactly:

- `trigger`, naming the explicit release workflow dispatch;
- `source`, naming the canonical repository and main ref;
- `manifest_inputs`, exactly the TOML and JSON compatibility pair;
- `payload_inputs`, exactly four rows sorted by Runtime artifact ID;
- `portable_subject_inputs`, exactly twelve rows sorted by artifact ID; and
- `evidence_inputs`, exactly seventeen rows sorted by source ID.

`internalParameters` is exactly `{}`.

`resolvedDependencies` is sorted by URI and digest and contains the repository
revision, pinned tool downloads, every locked source material, commit-pinned
workflow actions, digest-pinned OCI inputs, and all thirty-three aggregation
inputs at their logical `file:release-inputs/...` URIs. Every dependency uses
only its applicable `gitCommit`, `sha256`, or `sha512` digest algorithm.

## Aggregation result

The second `runDetails.byproducts` entry contains canonical JSON for
`worldstream/release-aggregation/v2`. It has exactly:

- `schema`, `operation`, `product`, `source_revision`, and `source_date_epoch`;
- `subject_count`, exactly `33`;
- `component_graph_sha256`, binding the locked dependency graph;
- `payload_producers`, exactly four independently built Runtime rows;
- `portable_subject_producers`, exactly twelve deterministic subject rows;
- `evidence_producers`, exactly seventeen typed evidence rows; and
- `toolchains`, the source-archive toolchain identity.

Runtime producer rows retain the complete v3 build identity: source revision,
materials, hosted runner facts, toolchains, compiler, arguments, UI commands,
OCI inputs, and derived build arguments.

Each portable subject producer row has exactly `artifact_id`, `subject`,
`subject_sha256`, `input_uri`, `producer_id`, `upstream_job`,
`configured_runner`, `aggregation_job`, and `aggregation_runner`. Its producer
ID is `starter-release-subjects/v1`; its upstream and aggregation route is the
Ubuntu `release-evidence` job. These rows attest deterministic wrapping and
copying, not independent compilation.

Each evidence producer row has exactly `source_id`, `evidence_id`,
`producer_id`, `subject`, `subject_sha256`, `input_uri`, `platform`, `checks`,
`upstream_job`, `configured_runners`, `aggregation_job`, and
`aggregation_runner`. The closed seventeen-source inventory includes the
original thirteen v3 sources plus official Pack/Component conformance,
Negotiate oracle/privacy/A202, SQLite released-artifact restart/Replay, and
PostgreSQL released-artifact restart/Replay.

## Run details and verification

`runDetails` has exactly `builder`, `metadata`, and `byproducts`. Its first
byproduct is the canonical GitHub-hosted runner identity. Its second is
`worldstream-release-aggregation-v2.json` with media type
`application/vnd.worldstream.release-aggregation.v2+json`. Each descriptor
binds the SHA-256 of its strict-base64 canonical JSON content.

An authorized operator dispatches the compatibility workflow on main with the
release input enabled. Local fixtures may exercise structural generation with
an explicit non-release invocation, but release verification requires the
canonical workflow, runner, source revision, exact thirty-three subjects, and
content-addressed v4 Type URI. Checkout-only diagnostics, narrative claims,
prototype output, unsigned qualification, and altered artifact bytes fail
closed.

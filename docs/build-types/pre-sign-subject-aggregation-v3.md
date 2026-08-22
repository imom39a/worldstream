# WorldStream pre-sign subject aggregation build type v3

This document defines version 3 of the WorldStream SLSA build type. Producers
identify this definition with the immutable Git blob URL for the commit that
first publishes this file. Any incompatible semantic or field change requires
a new definition, example path, and Type URI.

A complete illustrative statement is published at
[pre-sign-subject-aggregation-v3.example.json](https://github.com/imom39a/worldstream/blob/main/docs/build-types/pre-sign-subject-aggregation-v3.example.json).
The example is documentation, not release evidence or a resolved dependency.
It is published after this definition so this document can have an immutable,
non-self-referential Type URI.

## Build boundary

The build is the `release-evidence` job's pre-sign aggregation phase. It starts
with four already-produced payloads and thirteen already-produced typed source
reports. It validates their embedded identities, copies the exact input bytes
into the closed release layout, and emits an in-toto Statement whose seventeen
subjects are those byte-for-byte copies.

Signing occurs later. The signature bundle, final detached manifest, checksums,
SBOM, and provenance statement are neither inputs nor subjects of this build.
This build type does not claim that the aggregation runner compiled a payload
or made an upstream measurement.

## Closed statement shape

The statement has exactly `_type`, `subject`, `predicateType`, and `predicate`.
`_type` is `https://in-toto.io/Statement/v1`; `predicateType` is
`https://slsa.dev/provenance/v1`. `subject` contains exactly seventeen unique
rows sorted by `name`. Each row has exactly `name` and
`digest: {"sha256": HEX}` and corresponds bijectively to one payload or
evidence result row by path and digest.

`predicate` has exactly `buildDefinition` and `runDetails`.
`buildDefinition` has exactly `buildType`, `externalParameters`,
`internalParameters`, and `resolvedDependencies`. Unknown, missing, duplicate,
non-canonical, reordered where ordering is prescribed, or incorrectly typed
values are rejected.

## External and internal parameters

`externalParameters` has exactly:

- `trigger`: exactly `event`, `ref`, and `inputs`, with release values
  `workflow_dispatch`, `refs/heads/main`, and `{"release": true}`;
- `source`: exactly `repository` and `ref`, with values
  `https://github.com/imom39a/worldstream` and `refs/heads/main`;
- `manifest_inputs`: exactly `file:compatibility.toml` followed by
  `file:compatibility.json`;
- `payload_inputs`: exactly four rows, sorted by `artifact_id`, each containing
  exactly `artifact_id` and its `file:release-inputs/payload/...` URI; and
- `evidence_inputs`: exactly thirteen rows, sorted by `source_id`, each
  containing exactly `source_id` and its
  `file:release-inputs/source-reports/...` URI.

`internalParameters` is exactly `{}`.

## Resolved dependencies

`resolvedDependencies` is sorted by URI and canonical digest object. Every row
has exactly `uri` and `digest`. It contains:

- the canonical repository with exactly one `gitCommit` digest for the source
  revision;
- the SHA-256-pinned Zig archive and SHA-512 integrity-pinned pnpm tarball;
- every closed pinned source material used by the build-identity validator;
- every commit-pinned GitHub Action referenced by the workflow;
- the digest-pinned OCI base, BuildKit, and Dockerfile-frontend images; and
- all seventeen aggregation inputs at their logical file URIs.

A digest object uses only the algorithm applicable to that dependency:
`gitCommit`, `sha256`, or `sha512`. The example and this document are not added
again merely to describe the schema; the immutable v3 definition is already a
pinned material inside each payload identity.

## Aggregation result

The second `runDetails.byproducts` entry contains the canonical JSON encoding
of `worldstream/release-aggregation/v1`. Its decoded object has exactly:

- `schema`, equal to `worldstream/release-aggregation/v1`;
- `operation`, equal to `validate-and-copy`;
- `product`, the release version;
- `source_revision`, the exact 40-character lowercase Git commit;
- `subject_count`, exactly `17`;
- `component_graph_sha256`, a `sha256:` reference over the canonical locked
  Cargo, Python, pnpm, and Alpine component rows;
- `evidence_producers`, exactly thirteen rows;
- `payload_producers`, exactly four rows;
- `source_date_epoch`, the non-negative reproducible build epoch; and
- `toolchains`, the source-archive toolchain identity.

### Payload producer rows

Every payload row has exactly:

`artifact_id`, `subject`, `subject_sha256`, `input_uri`,
`build_identity_sha256`, `source`, `materials`, `target`,
`observed_build_environment`, `upstream_job`, `configured_runner`,
`aggregation_job`, `aggregation_runner`, `toolchains`, `compiler`, `arguments`,
`ui_commands`, `oci`, and `derived_build_arguments`.

The four artifact IDs are `source-archive`,
`native-linux-x86_64-archive`, `native-windows-x64-archive`, and
`oci-linux-amd64-image`. `subject_sha256` and `build_identity_sha256` are
`sha256:` references. `source` has exactly `repository` and `revision`.
`materials` is the exact closed path-to-`sha256:` map. `target` has exactly
`profile`, configured `runner`, and `triple`. `arguments` has exactly the
canonical `cargo` and `package` argument arrays. `ui_commands` is the exact
ordered list of command-token arrays exercised by that payload route.

`toolchains` is a closed name-to-object map. Each toolchain object has exactly
`version` and `pin`. Source and Windows payloads contain `node`, `pnpm`,
`python`, `rustc`, and `uv`; Linux additionally contains `zig`; OCI contains
those entries plus `buildx`.

`compiler` is `null` for the source archive. For a native or OCI payload it has
exactly `arguments`, `name`, `target_triple`, and `version`, with `name` equal
to `rustc` and arguments equal to the canonical Cargo build arguments.

`observed_build_environment` has exactly `runner`, `rustc`,
`bundled_sqlite`, and `final_linker`. `runner` has exactly `provider`, `os`,
`architecture`, `image`, and `image_version`. The source archive uses `null`
for all three native-tool fields. Every native-tool object has exactly `path`,
`version`, `target`, and `reported_target`; native paths are absolute and the
reported targets must match the trusted hosted route. `bundled_sqlite` has
exactly `archiver` and `c_compiler`. The archiver field represents GNU archive
creation on Linux/OCI and the x64 COFF librarian on Windows; it must never be
inferred from the final linker. `final_linker` independently identifies the
linker that produced the native executable.

`oci` is `null` for non-OCI payloads. For the OCI payload it has exactly
`base_image`, `buildkit_image`, `dockerfile_frontend`, `build_arguments`, and
`platform`. Its build arguments exactly bind manifest digest, epoch, revision,
version, base image, and the base64 observed-build-environment label.
`derived_build_arguments` is `{}` for non-OCI payloads and exactly
`BUILD_IDENTITY_SHA256` plus `BUILD_ENVIRONMENT_BASE64` for OCI.

Configured runner routes and observed runner facts are distinct. Every payload
row names its upstream job and configured runner, while
`observed_build_environment.runner` records what the upstream build actually
observed. `aggregation_job` and `aggregation_runner` are always
`release-evidence` and `ubuntu-24.04`.

### Evidence producer rows

Every evidence row has exactly:

`source_id`, `evidence_id`, `producer_id`, `subject`, `subject_sha256`,
`input_uri`, `platform`, `checks`, `upstream_job`, `configured_runners`,
`aggregation_job`, and `aggregation_runner`.

`checks` is a sorted, non-empty list. `configured_runners` is an ordered,
non-empty route list and is not an observed-host claim. The source IDs are the
closed thirteen-source manifest inventory; their subjects are the matching
typed evidence IDs under `supply-chain/subjects/`.

## Run details and byproducts

`runDetails` has exactly `builder`, `metadata`, and `byproducts`.
`builder` is exactly `{ "id": URI }`; for release evidence its ID is the
canonical main-branch workflow ref. `metadata` is exactly
`{"invocationId": URI}` naming the GitHub Actions run attempt. Whole-job start
and finish times are omitted because the generator does not observe trustworthy
values for that boundary.

`byproducts` contains exactly two Resource Descriptors in this order:

1. `worldstream-runner-identity.json`, media type `application/json`; and
2. `worldstream-release-aggregation-v1.json`, media type
   `application/vnd.worldstream.release-aggregation.v1+json`.

Each descriptor has exactly `name`, `digest: {"sha256": HEX}`, `mediaType`, and
`content`. `content` is strict base64 of canonical JSON and its digest is the
SHA-256 of the decoded bytes. The runner object has exactly `provider`, `os`,
`architecture`, `image`, and `image_version`; a release statement requires the
observed GitHub-hosted Ubuntu aggregation runner. The aggregation descriptor
must decode to the exact graph recomputed from the seventeen statement
subjects and four embedded payload build identities.

## Initiation and verification

An authorized operator dispatches **Compatibility gates** on
`refs/heads/main` with `release=true`. The aggregation begins only after all
four payloads and thirteen typed reports exist. Local fixtures may exercise
the structure with the canonical local-test invocation and runner identity,
but release verification rejects those values. Validation always recomputes
the graph from exact subject bytes, embedded payload identities, source
materials, workflow routes, and producer reports; the illustrative example is
never accepted as evidence.

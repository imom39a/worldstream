# WorldStream pre-sign subject aggregation build type v5

This document defines the active CLI-first WorldStream SLSA build type. Its
Type URI is `urn:worldstream:build-type:sha256:<sha256-of-this-file>`. Version 4
remains valid only for the historical, implicit
`worldstream/release-inventory/runtime-packs-studio-v1` inventory.

The build is the `release-evidence` job's unsigned pre-sign aggregation phase.
It validates and copies a closed graph produced by independent jobs. It does
not claim that the aggregation runner compiled the Runtime or generated an
evidence result.

## Inventory contract

The external parameters contain `release_inventory` with the exact value
`worldstream/release-inventory/cli-first-v1`. The graph contains exactly 32
subjects:

- four Runtime/source payloads;
- eleven portable Starter subjects; and
- seventeen non-supply-chain source reports.

The eleven portable subjects are the historical set except
`worldstream-studio`. `worldstream-participant-console` remains and carries the
standalone Activity Client Host and first-party client payload. All evidence
subjects remain unchanged.

The signed pre-sign inventory uses
`worldstream/release-subject-inventory/v2` and repeats the exact
`release_inventory` value. The detached release manifest uses
`worldstream/release-artifact-manifest/v3` and repeats it again. A v1 inventory,
v2 detached manifest, missing discriminator, extra Studio subject, or signature
over a different inventory is not interchangeable with this profile.

## Native payload contract

Each CLI-first Linux or Windows native archive contains these five binaries
(with `.exe` suffixes on Windows):

- `worldstreamd`;
- `worldstreamctl`;
- `worldstream-studio-supervisor` (retained headless compatibility name);
- `worldstream-assignment-mcp`; and
- `worldstream-managed-agent-host`.

The Supervisor name does not identify or include the retired Studio web
product. Source and OCI payload contracts remain otherwise unchanged.

## Aggregation result

The second SLSA `runDetails.byproducts` descriptor is named
`worldstream-release-aggregation-v3.json`, has media type
`application/vnd.worldstream.release-aggregation.v3+json`, and contains
canonical JSON with exactly these fields:

- `schema`, equal to `worldstream/release-aggregation/v3`;
- `release_inventory`, equal to the CLI-first inventory identity;
- `operation`, equal to `validate-and-copy`;
- `product`, `source_revision`, `subject_count`, and
  `component_graph_sha256`;
- `payload_producers`, `portable_subject_producers`, and
  `evidence_producers`;
- `source_date_epoch`; and
- `toolchains`.

The producer rows, materials, toolchain observations, workflow actions,
runner identity, source revision, and invocation requirements are otherwise
the same closed contracts defined by build type v4. The aggregation descriptor
and SLSA `subject` list cover the exact same 32 paths and SHA-256 digests.

## Verification

A verifier must select the build type, detached-manifest schema, subject-
inventory schema, aggregation schema, discriminator, and exact subject set as
one versioned contract. It must reject cross-version substitution even when
the common payload bytes happen to be identical. Structural validation is not
signature verification; the signature must authenticate the exact canonical
v2 subject-inventory bytes before final assembly.

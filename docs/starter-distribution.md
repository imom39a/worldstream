# Starter Distribution

The WorldStream Starter Distribution is one deterministic, self-contained
carrier for the pieces an Application Integrator needs to start without
rebuilding the Runtime Distribution. It is not a second Runtime Distribution,
a Pack registry, or an approval authority.

## Trust model

The Starter has two layers:

```text
signed release-manifest.json
  └─ exact release subjects (Runtime, UIs, SDK/toolchain, adapters, docs, legal)
       └─ deterministic Starter carrier
            └─ optional exact custom .wspack candidates
```

The detached WorldStream release manifest and its Sigstore bundle authenticate
every official release subject. The Starter manifest binds each carried file by
SHA-256 and byte count to that signed inventory. The carrier's compression is
deterministic and receives its own SHA-256 in the verification receipt, but it
does not create a second signing cycle. Offline verification authenticates the
embedded release manifest, then checks the closed subject inventory and every
byte.

An official Starter accepts only release-bound subjects. A custom Starter keeps
the exact same signed Runtime and tooling subjects and may add proved Activity
Pack Bundle/evidence pairs. A custom Pack is identified by its physical BLAKE3,
semantic revision BLAKE3, descriptor Pack ID, and explanatory version. It is
still a candidate: installation requires a Host Operator to approve that exact
physical bundle digest on the target installation. No approval record moves
with the carrier.

## Closed contents

An official v1 Starter contains exactly one subject for each of these roles:

- Runtime Distribution;
- first-party independent Activity Client Host and client assets;
- TypeScript Pack SDK and pinned Pack Toolchain;
- official immutable WorldStream Negotiate bundle and conformance evidence;
- A202 adapter and independent Negotiate evidence verifier;
- deterministic agent fixtures and examples;
- documentation, including the outside-adopter checkpoint kit, licenses, and
  release metadata.

The A202 adapter subject must carry the closed
`interop/a202-peer-v1/` invitation, independent-receipt validator, and
incomplete receipt template. The documentation subject must carry the closed
`qualification/outside-adopter-v1/` clean-environment kit. These are released
specification and observer tools, not independent participants or release
evidence. Packaging a local vector never satisfies an interoperability or
adoption evidence gate.

Each subject is an already-built file in the detached release inventory. A
directory must first be converted to a deterministic release artifact; the
Starter builder never walks a working tree, follows a symlink, downloads a
dependency, or infers an unlisted file. This keeps the input reviewable and
lets the same runtime archive bytes appear in both official and custom
candidates.

The current release workflow selects the CLI-first inventory explicitly.
Omitting the selector remains supported only to verify the historical
Studio-inclusive contract. A trusted v3 detached manifest
selects `worldstream/release-inventory/cli-first-v1`; its closed Starter role
set omits `studio` but still requires `participant-console` for the standalone
Activity Client Host and clients. A Starter cannot relabel a v2 manifest, add
the retired web application to the CLI-first role set, or treat a signature
over the historical pre-sign inventory as successor coverage.

The carrier rejects extra or duplicate members, links and special files,
absolute or traversing paths, non-canonical ownership/modes/timestamps,
substituted bytes, unlisted release subjects, Pack member drift, and paths that
name approvals, credentials, private keys, databases, backups, or secrets.
The generated manifest records that it carries no approval state, credential,
or mutable database state and requires neither a registry nor a network
download.

## Candidate inventory

The builder consumes strict JSON using
`worldstream/starter-candidate/v1`. This abbreviated example shows the two
binding forms; a real official inventory must contain the complete role set.

```json
{
  "activity_packs": [
    {
      "bundle_digest": "blake3:<physical-bundle-digest>",
      "bundle_subject_id": "worldstream-negotiate",
      "evidence_subject_id": "worldstream-negotiate-evidence",
      "explanatory_version": "0.1.0",
      "official": true,
      "pack_id": "worldstream.negotiate",
      "revision_digest": "blake3:<semantic-revision-digest>"
    }
  ],
  "distribution": {
    "id": "worldstream-starter",
    "mode": "official",
    "profile": "native-linux-x86_64",
    "version": "0.1.0"
  },
  "schema": "worldstream/starter-candidate/v1",
  "subjects": [
    {
      "binding": {
        "id": "native-linux-x86_64-archive",
        "inventory": "artifact",
        "kind": "release"
      },
      "id": "runtime-linux",
      "license_expression": "Apache-2.0",
      "media_type": "application/gzip",
      "role": "runtime-distribution"
    }
  ],
  "trust": {
    "compatibility_manifest": "release/compatibility.json",
    "release_manifest": "release/release-manifest.json",
    "sigstore_bundle": "release/sigstore.bundle.json"
  }
}
```

A release binding has no caller-selected source path. The builder resolves the
path and SHA-256 from the signed release manifest. Only a custom
`activity-pack-bundle` or `activity-pack-evidence` subject may use a candidate
binding with a local file path. The complete Linux role inventory is checked in
as `packaging/starter/official-candidate.example.json`; the expanded release
gate owns production of the correspondingly named detached subjects.

## Build and verify

Build from an already assembled, signed release directory:

```sh
scripts/starter-distribution.py build \
  --inventory starter-candidate.json \
  --output worldstream-starter-0.1.0-linux-x86_64.tar.gz \
  --source-date-epoch "$SOURCE_DATE_EPOCH"
```

Verify later without a registry or source checkout:

```sh
COSIGN_CERTIFICATE_IDENTITY='<release workflow identity>' \
COSIGN_CERTIFICATE_OIDC_ISSUER='<trusted issuer>' \
scripts/starter-distribution.py verify \
  worldstream-starter-0.1.0-linux-x86_64.tar.gz
```

The verifier uses the same configured Cosign identity policy as the detached
release verifier. `--structural-only` is available for artifact development
and returns exit code 11; it validates all deterministic structure, exact
digests, Pack identities, and release bindings but deliberately does not claim
that the Sigstore identity was verified.

After verification, the Host Operator still uses `worldstreamctl pack inspect`,
`approve`, `install`, and `restart-readiness`. Extracting or verifying a Starter
never writes local approval state, changes a Room, edits a database, or makes a
Pack selectable.

## Current release frontier

The repository compatibility pair remains a fail-closed specification with
`release_ready = false`. Therefore a cryptographically verified official
Starter cannot yet be emitted from the checkout. The builder and offline
verifier are complete and adversarially tested. The active CLI-first release
workflow now deterministically builds and re-verifies all eleven portable
subjects and includes them in the 32-subject pre-sign inventory. It must still
produce the native/OCI/source artifacts, obtain every genuine Runtime/Pack
report, pass the existing supply-chain and platform evidence, and receive both
detached signatures before an official carrier can exist.

Starter recovery is a post-sign qualification. After an official and custom
Starter both authenticate the same primary release, their exact verification
receipts are bound by `scripts/release-qualification.py` into a separately
signed `release-qualification-manifest.json`. Structural-only receipts do not
qualify, and no Starter or qualification generator changes `release_ready`.

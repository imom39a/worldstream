# IMO-59/60/61 Wave 6: deterministic manifest and release-tooling audit

Date: 2026-08-21
Lane: parent-orchestrated Wave 6
Scope: read-only identity/evidence plumbing for the SQLite bundle, migrations,
Agent Heist pack, manifest parity, evidence digests, and release inventory.

## Outcome

Wave 6 adds a dependency-free report/validation command at
[`scripts/manifest-evidence-wave6.py`](../../scripts/manifest-evidence-wave6.py)
and focused tests at
[`tests/manifest_evidence_wave6.py`](../../tests/manifest_evidence_wave6.py).
The command only reads source and manifest bytes. It does not edit
`compatibility.toml`, `compatibility.json`, create a release directory, or
promote implementation identities to release evidence.

The checked-in manifest remains `manifest_kind = "specification"` and
`release_ready = false`.

## Exact command output

The report was generated against the pinned rusqlite checkout at revision
`229140734a4a60cc9fa34507fe79cb2277142f49`:

```text
$ python3 scripts/manifest-evidence-wave6.py --bundled-checkout /Users/vinothshanmugam/.cargo/git/checkouts/rusqlite-8e5e8204e489f2fd/2291407
manifest evidence wave6 validation: pass
manifest parity: {'semantic_equal': True, 'canonical_mirror_equal': True, 'reviewed_source_ok': True, 'canonical_mirror_ok': True}
SQLite identity consistent: True
SQLite migration bodies: 6
Agent Heist manifest status: unresolved_by_design
release artifact inventory: implementation shape verified; digests remain unresolved
```

The machine-readable output was 13,155 bytes and had:

```text
json_stdout_sha256=sha256:1e904f31d787f4f5263d3f4c1c018a9326b663dd42a798d1ba5aff3382bfd8e5
json_stdout_ends_newline=True
```

Focused and existing checks:

```text
$ python3 -m unittest -q tests/manifest_evidence_wave6.py
Ran 4 tests in 0.852s
OK

$ python3 scripts/verify-manifest.py
compatibility manifest verified (specification-only, release_ready=false)

$ cargo test --locked -p worldstream-core --lib registry_golden_transcript_is_fixed
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 141 filtered out

$ cargo test --locked -p worldstream-sqlite --lib opens_only_the_exact_bundled_sqlite_engine
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 83 filtered out

$ cargo test --locked -p worldstream-postgres --lib migration
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 18 filtered out

$ cargo test --locked -p worldstream-runtime --lib embedded_manifest_is_valid_and_fail_closed
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 30 filtered out
```

`tests/package_smoke.py` also passed with its existing intentional negative
fixture (`GATE FAIL path-safety: unsafe release path: '../outside'`) and ended
with `package archive smoke passed`. That fixture failure is part of the test,
not a release claim.

## SHA-256 and BLAKE3 inventory

All `sha256:` values below are hashes of exact bytes read by the report
command. BLAKE3 values are recomputed by the report's unkeyed BLAKE3
implementation where the Rust source defines the input bytes explicitly.

### Manifest parity

| Input | Bytes | SHA-256 |
|---|---:|---|
| `compatibility.toml` | 18,767 | `sha256:6a441510c2a9ba2b358767403af42d73582add64ca77a9fd50dc2b6ce532f012` |
| `compatibility.json` | 23,632 | `sha256:62897e5fcba6af664159acbe99d7ff305d062fd4b2c27231f99c8420745cc200` |

The authored TOML and JSON are semantically equal, the JSON is the sorted
canonical rendering, and both source/mirror metadata pointers are correct.

The release inventory has the eight IDs defined by `scripts/package.py`, in
the same order and with matching profiles. All eight manifest artifact
digests are empty/unresolved. No release directory was created.

### SQLite engine identity

The source constants and Cargo pins agree:

```text
SQLITE_VERSION=3.53.4
SQLITE_SOURCE_ID=2026-07-24 19:02:57 bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc
RUSQLITE_BUNDLE_REVISION=229140734a4a60cc9fa34507fe79cb2277142f49
```

`crates/worldstream-sqlite/src/lib.rs` is 926,577 bytes with
`sha256:2a1bb4f29e378b72371a2ebc80c1d9a6601f8e4fc29b627f7e0fc7087db13426`.
The pinned bundled source input inventory is 535 canonical inventory bytes
with aggregate SHA-256
`sha256:3c9f9560f759ff0b8bedf2714f68d0bf6f06bf5a7dec2ce43ba833d424cbe4d6`.

| Bundled input | Bytes | SHA-256 |
|---|---:|---|
| `libsqlite3-sys/build.rs` | 34,775 | `sha256:0346b3e8ee797df02166416a199b199a5860a94c7f5a08da27e3c2bc0c166c10` |
| `libsqlite3-sys/sqlite3/bindgen_bundled_version.rs` | 126,498 | `sha256:59b073d337482286fb0e1b009e2800b70e6ce84b839c089b5cfa5e8059d44e4e` |
| `libsqlite3-sys/sqlite3/sqlite3.c` | 9,515,341 | `sha256:b1dd5d74ec7f29055a6684fa06fb3c2f6821c87dd38f9a458dfd2e8a1db28189` |
| `libsqlite3-sys/sqlite3/sqlite3.h` | 690,838 | `sha256:919e7f2e8ed1d8f56ac17b412b8971c76aa5d1a879752cc6058f75e7d5910e1d` |
| `libsqlite3-sys/sqlite3/sqlite3ext.h` | 39,175 | `sha256:ac9645e5c9ff0cf176efdd6e75cb5e98f46295d38e02db5c4d208826a39ab4be` |

This is a reproducible source-input inventory, not the missing
`storage.sqlite.bundle_build_digest`: the latter must identify an actual
reviewed release build and remains unresolved.

### Migration bodies

The SQLite adapter contains six forward migration bodies. It does not expose a
SQLite migration checksum function or persist SQLite checksum fields; the
report therefore emits both useful byte hashes and the explicit
`checksum_implementation = not present in SQLite adapter` finding.

| SQLite migration | Bytes | SHA-256 | BLAKE3 |
|---|---:|---|---|
| `0001-initial-storage-schema` | 6,744 | `sha256:fd4563ff1ed935d6be24bf797edf3a5e2c440d7d25d38cb620b738f4f719d78f` | `blake3:dd07208c71d7165b93861883b25411b1e7c33a6be36fc2be28a638e1ab5cd763` |
| `0002-operational-authority-v1` | 7,440 | `sha256:d3e25ae210b17948e782a20800668255822d1418fd20bb1a9ea0ceb195fbbd48` | `blake3:237088a0f888ef9f91a1010efd95e38a40170b0fc968229b886881937af805b0` |
| `0003-observation-delivery-v1` | 1,369 | `sha256:7ab262e5d7e5fc8f9ed4fed927176dd33f05e849e995bba96c1c4753c6bd0106` | `blake3:b74d06ed529d415a658eaede5067f24e02ff5de83ac07480a5df7de1645c8bcb` |
| `0004-activation-work-v1` | 3,510 | `sha256:b7ea565c9b75ce679af43deafce891e56458f4a34499d3afed13945752fdb8fc` | `blake3:dbe807e620fc77594e871b1ad90e89379498060fd025fbbaa318396158557d2b` |
| `0005-paired-snapshots-v1` | 838 | `sha256:de99bfbfc4dfc65a0900161f2cc2971c453a88cf16b29899e1250b5cb3bdc2df` | `blake3:385af50337813e01ce0a97894fcb82868e130d69e671f64712f6701c0e9ddb23` |
| `0006-canonical-export-metadata-v1` | 278 | `sha256:ed3926961f78b43445e6c2dcb2c100eb7e83061f231ed66a0424a694978cf398` | `blake3:60de4825b3796865acff18f836dfa475640324b71d168350a8ea20c2e06206d5` |

The PostgreSQL source contains seven migration descriptors. Its
`MigrationDescriptor::checksum` computes BLAKE3 over each SQL body. The first
three recomputed values exactly match the three populated PostgreSQL
manifest checksums:

| PostgreSQL migration | Bytes | SHA-256 | BLAKE3 |
|---|---:|---|---|
| `0001-initial-storage-schema` | 2,570 | `sha256:02735d7205c03e84ee958529d78707db378ccfc4697c49b9a5e933b27ebbf3ff` | `blake3:cda5b4edaed76bd2beac2df1f5cd89dbcdcf72fd2424dc4576042370f61471a6` |
| `0002-operational-authority-v1` | 437 | `sha256:9bfb56b116f50cc93037ba2909acef6bab5eab94047cfc9246b97ecc8cc7e0d9` | `blake3:b8f4ec2e2d47de2d9979c21505da698fc9b768f53ea4e8ef9429b8f042be9175` |
| `0003-kernel-conformance-v1` | 4,385 | `sha256:571c9d751b51ff853ee5d3a3a6557c8b82c7408d929869752bfeb90ec74fb97f` | `blake3:cc5bd1a11223932ebe239ce7de97d88d6390b8651f90f118bf069d3f3798da0d` |
| `0004-kernel-parity-witnesses-v1` | 746 | `sha256:c02f68a25ba4186e5594858904a6899906e31961e31c358630c545b990b7cb6f` | `blake3:eba8944ea435d27e737a22164ed314fc62d08a76eb068e5c7eedb9ae2fc6d4ea` |
| `0005-transfer-publication-v1` | 722 | `sha256:cdefb1bf632879b7f1ddeab126c808c91d9cf4794713c5b9cf1a909681527c22` | `blake3:a2d43f76d7986a17d8a975be9f0bcbc2dcf16290b01b08dd5ef606f21ffc2cea` |
| `0006-transfer-target-fence-v1` | 272 | `sha256:b6ae1fbac6f41c5dea1ca1b6a22a81cddf8e39b1e3dbb1a70aa63253e9fb810d` | `blake3:7a0b8e471f67d33d6610bb39b96fb64bd1a0cd56a3d1557ad2f843e89d0078d5` |
| `0007-deployment-metadata-v1` | 360 | `sha256:a8b6c22e9eda590e0486125ca248ac0a82393b43b359e59d2b109254c207973a` | `blake3:ebc57f9b917e3655f1753c5543d517f8642d5ff9de43ef0cc7b3795d9c43dc03` |

The manifest currently covers only PostgreSQL migrations 1–3; source IDs 4–7
are not in the manifest. SQLite migration checksums and the missing prior
PostgreSQL rows remain release-gated work, not values this lane may insert.

### Agent Heist pack identity

The Rust source computes the executor artifact as:

```text
blake3(canonical_text_artifact(include_bytes!("agent_heist.rs")))
```

The normalized input is 80,955 bytes:

```text
source_sha256=sha256:323acbbe532cfca7c9f27577d4d4750e56b6978e997f12e271f2d55293f33348
executor_input_blake3=blake3:aa25d60ab8a60c2d52994f7d77d0219f8a543a4bc3fa34bed8362289ce208916
registry_source_sha256=sha256:d3aaecd6ccbdd69c48284831750f9ac0f354a7e9d144a0da1d421dc2b406045b
```

The registry's fixed transcript literal is
`blake3:33949bd5b664af05341193f8721ed9abe801de28fdb9b8a5b55176bd5c1f9ec8`.
`registry_golden_transcript_is_fixed` passed, proving the compiled registry
recomputes and checks its complete golden transcript. The manifest's Agent
Heist row remains unresolved for revision, executor, descriptor, schema,
codec, and corpus fields; this lane deliberately does not copy source-derived
values into it.

## Evidence classification

Implementation-verifiable in this lane:

- authored TOML/JSON semantic and canonical parity;
- SQLite version, source ID, rusqlite revision, and Cargo.lock agreement;
- deterministic SHA-256 inventory of the pinned SQLite source inputs;
- exact SQLite and PostgreSQL migration body bytes and recomputed BLAKE3 values;
- Agent Heist executor source formula/input digest and fixed-registry test;
- release artifact inventory shape, ordering, profiles, and unresolved status;
- report stdout SHA-256.

Still external or not proven by this lane:

- a reviewed release-build `storage.sqlite.bundle_build_digest`;
- SQLite persisted migration checksum implementation/evidence;
- manifest completion for PostgreSQL migrations 4–7;
- Agent Heist manifest identity values and release artifact digests;
- native Linux/Windows/OCI artifacts;
- signatures, SBOM, provenance, and their artifact digests;
- crash/power-loss, full transfer/restore, provider, and one-hour soak evidence;
- the already-recorded live PostgreSQL evidence remains an existing manifest
  row and was not regenerated or altered by Wave 6.

No `release_ready` field, manifest value, release artifact, or evidence claim
was manufactured.

## Parent gate boundary

The full pre-push gate was run after the Wave 6 checks. It exited 1 with the
following exact summary:

```text
Gate summary: 56 checks, 17 failures, 1 incomplete skips
```

The Wave 6-specific command, manifest verification, focused Rust identity
tests, SDK tests, UI checks, shell syntax, and package smoke passed. The
additional pre-push failures are outside the two Wave 6 code files and were
not changed in this lane:

- `crates/worldstream-core/src/registry.rs` formatting and two Clippy docs;
- `crates/worldstream-server/src/lib.rs` / `sqlite_backend.rs` existing test
  compilation errors in the telemetry evidence slice;
- `examples/heist/wave6-live/run_live_story.py` existing Python format/lint
  findings;
- unresolved release-gated evidence rows, which remain intentionally
  fail-closed.

The full gate therefore does not establish release readiness and was not
silenced or repaired by changing unrelated lanes.

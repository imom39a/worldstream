# IMO-59 release artifact implementation lane

Date: 2026-08-21

This lane changed only release packaging validation and its focused smoke test.
The compatibility manifest pair remains untouched and still reports
`manifest_kind = "specification"` and `release_ready = false`.

## Changes

- `scripts/package.py` now validates the daemon Cargo package version, Python
  SDK version, and console UI `package.json` version against the manifest
  product version before native or OCI packaging writes output. The check is
  fail-closed and is also available through the source-package path.
- Archive verification now rejects payload paths outside the documented
  target layout. Existing ordinary-file, traversal, duplicate, symlink,
  special-member, timestamp, ownership, and permission checks remain active.
- `scripts/package.py` now derives a canonical client-contract identity from
  `compatibility.json`: wire/config/storage/core/hash fields and the complete
  exact `pack_executors` row set, including all artifact/revision digests and
  selectable/retained/status policy. Packaging requires matching deterministic
  identity artifacts in the Python SDK and UI, and archive/OCI verification
  independently checks them against the archive/context manifest.
- The SDK exposes the embedded identity through
  `worldstream_sdk.CLIENT_CONTRACT_IDENTITY`; the UI exports the embedded
  public identity through one raw-imported artifact, and production `main.tsx`
  exposes the parsed object on `window.__WORLDSTREAM_CLIENT_CONTRACT_IDENTITY__`.
  `package_smoke.py` covers positive
  identity validation plus wire/config drift, missing and stale pack rows,
  changed exact revision digest, malformed identity, and SDK/UI disagreement.
- Added manifest-derived SDK/UI identity artifacts without changing
  `compatibility.toml` or `compatibility.json`.
- Added fail-closed validation for the UI-imported retained-Heist parity
  fixture and embedded UI `pack_digest` references. The smoke test rejects
  the orphan digest `blake3:755aa7a88b8236d951da297e700ab41501d091a0ee1585547e90d8a46da95bfe`.
- Removed the duplicate UI `src/compatibility_identity.json`. Source archive
  validation now requires the UI module's raw import from
  `public/compatibility-identity.json` and its production entrypoint exposure;
  native/OCI validation requires the exact canonical identity bytes in the
  built production bundle. Negative smoke cases mutate consumed wire/config/
  full pack-row identity while leaving the public artifact valid.
- `tests/package_smoke.py` now exercises UI version drift, POSIX
  group/world-writable input rejection, Windows separator/UNC-style path
  rejection, unlisted archive paths, and canonical archive modes.
- `docs/release-packaging.md` documents the version identity and layout gates.

## Commands and results

Passed:

```text
python3 tests/package_smoke.py
uv run --project sdk/python --locked pytest sdk/python/tests/test_skeleton.py
cd web/console && npm test -- --run src/compatibilityIdentity.test.ts
npm --prefix web/console run build
uv run --project sdk/python --locked pytest sdk/python/tests
uvx ruff check scripts/package.py tests/package_smoke.py sdk/python/src/worldstream_sdk sdk/python/tests
uvx ruff format --check scripts/package.py tests/package_smoke.py sdk/python/src/worldstream_sdk sdk/python/tests
python3 tests/oci_runtime_smoke.py
python3 tests/gate_smoke.py
python3 -m py_compile scripts/package.py tests/package_smoke.py
bash -n scripts/package-release.sh scripts/package-oci.sh scripts/verify-release.sh scripts/oci-runtime-smoke.sh scripts/macos-source-quickstart.sh
uv run --project sdk/python --locked ruff check scripts/package.py tests/package_smoke.py
uv run --project sdk/python --locked ruff format --check scripts/package.py tests/package_smoke.py
git diff --check -- scripts/package.py tests/package_smoke.py
SOURCE_DATE_EPOCH=0 scripts/package-release.sh --target linux-x86_64 --dry-run
SOURCE_DATE_EPOCH=0 scripts/package-release.sh --target windows-x64 --dry-run
SOURCE_DATE_EPOCH=0 scripts/package-oci.sh --dry-run
python3 scripts/verify-manifest.py
```

The package smoke creates synthetic release-valid inputs only. It verified
reproducible Linux tar and Windows zip archives, version/layout/checksum
identity, manifest-derived SDK/UI contract identities, ordinary archive
members, retained-Heist fixture identity, and a reproducible OCI context. It did not create a repository
release artifact or alter release evidence.
The current corrected retained-Heist fixture and rebuilt UI bundle also pass
the orphan-digest check; the negative smoke case rejects the prior
`755aa7...` digest.
The UI identity test suite passed 2 tests, the full UI suite passed 47 tests,
and the production UI build passed. The exact parent Ruff check and format
check both passed.

The Docker-backed cross-platform probe also passed its disposable Alpine
`linux/amd64` emulation check on the macOS arm64 Docker host. That is container
platform evidence only, not native Linux release evidence. The checked-in OCI
template runtime smoke returned `INCOMPLETE` with
`pinned_base_image_required`, as designed.

## Remaining blockers / explicit exclusions

- The native Linux x86-64 archive/runtime/permissions cell remains unproven on
  this macOS arm64 host; it requires the declared native Linux runner.
- The native Windows x64 archive, ACL/reparse behavior, filesystem, and
  runtime cell remains unproven; the path assumptions tested here are only
  representative negative checks, not Windows evidence. PowerShell was not
  available locally.
- OCI release evidence still needs a release-selected pinned base image,
  generated context, Linux/amd64 image build, non-root/read-only-root runtime,
  writable `/var/lib/worldstream` volume, image digest, and retained probes.
- macOS remains source-only and best-effort. The quickstart may run on
  Darwin/APFS with the pinned toolchain; it produces no macOS binary,
  installer, signing, or notarization claim. This host's ambient shell lacks
  the pinned Node/pnpm path, so no fresh quickstart pass is claimed here.
- The repository's local release contract still includes UI, SDK, examples,
  licenses, metadata, and checksums in native archives; this lane enforced
  that documented allowlist and did not delete those declared payloads.
- No compatibility digest, `release_ready` field, release manifest, signing
  bundle, SBOM, provenance, or Linear status was changed.

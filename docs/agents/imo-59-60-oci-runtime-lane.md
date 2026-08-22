# IMO-59/60 OCI runtime lane

This lane adds `scripts/oci-runtime-smoke.sh`, an executable verifier for an
already-generated OCI context. It builds with the context's exact pinned
`base_image`, forces Linux/amd64, inspects the image configuration, and runs a
read-only-root container as UID/GID `65532:65532` with an explicit
`/var/lib/worldstream` volume. It verifies the configured `worldstreamctl`
healthcheck, writes a marker into the persistent volume, and checks that the
entrypoint rejects a tmpfs data volume and a non-canonical data directory.

`tests/oci_runtime_smoke.py` executes the checked-in entrypoint policy against
all denied filesystem names (`overlay`, `tmpfs`, NFS, CIFS, FUSE variants, and
SMB), plus wrong-path, unknown-profile, PostgreSQL-profile, stat-failure, and
allowed-ext4 cases. It also asserts the packaging Dockerfile policy, rejects
the unresolved template metadata as incomplete, and preserves the manifest
gate's explicit boundary.

Run the static/policy lane with:

```sh
python3 tests/oci_runtime_smoke.py
```

Run image/runtime evidence against a generated context with:

```sh
scripts/oci-runtime-smoke.sh --context dist/oci
```

The runtime command is fail-closed and emits one stable JSON diagnostic with
`release_evidence: false`. Exit `10` is `UNAVAILABLE` for Docker, BuildKit, or
Python; exit `12` is invalid context configuration; exit `13` is `INCOMPLETE`
for unresolved or invalid generated metadata (including the checked-in
placeholder template); exit `14` is a failed image/runtime policy probe. A
local Docker volume whose filesystem is outside the ext4/xfs allow-list is
also `INCOMPLETE`, never a pass. Only a complete image/configuration/runtime
run emits `PASS` with exit `0`.

The entrypoint accepts only the declared `sqlite-bundled` and
`postgres-primary` profiles. SQLite requires a real, canonical
`/var/lib/worldstream` volume and ext4/xfs; missing filesystem inspection,
symlinked mounts, unknown profiles, tmpfs, and other filesystems all stop
startup with policy exit `78`. PostgreSQL persistence remains external to the
image and therefore does not claim SQLite filesystem evidence.

The runtime lane validates generated metadata identity, target, non-root and
read-only policy, healthcheck, volume, and filesystem lists before invoking
Docker. It does not resolve the checked-in placeholder base-image digest or
make any signing claim. The existing manifest gate remains fail-closed and
continues to require native Linux/amd64 release CI to provide the pinned
base-image digest, final image digest, signed release bundle, and retained
runtime evidence. Native Windows evidence, provider-native restore,
power-loss/native Linux durability, and release signing/SBOM/provenance remain
external requirements.

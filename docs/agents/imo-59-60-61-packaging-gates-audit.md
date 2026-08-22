# IMO-59/60/61 packaging and gate plumbing audit

Date: 2026-08-21

This audit is limited to packaging, OCI runtime policy, release verification,
and compatibility-gate plumbing. The checked-in compatibility pair remains a
specification (`manifest_kind = "specification"`, `release_ready = false`).
No placeholder artifact digest, OCI base-image digest, signing identity, SBOM,
or provenance claim was resolved.

## Implemented boundaries

- OCI entrypoint policy now accepts only `sqlite-bundled` and
  `postgres-primary`; SQLite requires a canonical `/var/lib/worldstream`
  volume and an `ext4`/`xfs` filesystem. Unknown profiles, missing filesystem
  inspection, symlinked mounts, and denied filesystems fail with policy exit
  `78`.
- OCI runtime smoke emits stable JSON diagnostics with distinct unavailable,
  configuration, incomplete, and runtime-failure exits. Unresolved generated
  metadata is `INCOMPLETE` with `release_evidence: false`.
- Archive verification binds the filename, top-level root, embedded target and
  version, manifest bytes, canonical metadata, checksums, and source-date
  epoch. Package reports are exact byte-identity reports and cannot overwrite
  or be written inside the artifact they describe.
- Gate reports preserve `pass`/`failure`/`incomplete` classification, include
  `status`, `fail_closed`, and `incomplete_blockers`, and identify exact
  release artifact/platform blockers. Strict and release tiers remain blocking
  for incomplete evidence.

## Parent verification

```text
/opt/homebrew/bin/python3 tests/package_smoke.py       PASS
/opt/homebrew/bin/python3 tests/oci_runtime_smoke.py   PASS
uv run --project sdk/python --locked pytest -q tests/gate_smoke.py  16 passed
ruff check (five scoped Python files)                  PASS
ruff format --check (five scoped Python files)         PASS
bash -n (scoped shell wrappers)                       PASS
```

The strict fast gate reports `20 checks, 8 failures, 0 incomplete skips`; all
eight failures are unresolved release artifact identities. The pinned-toolchain
release gate reports `59 checks, 28 failures, 0 incomplete skips` and remains
`status=blocked`, `fail_closed=true`. The remaining failures include unresolved
manifest/evidence/artifact identities, a missing release bundle, and an
out-of-scope dirty-worktree compile failure in `worldstream-backup`.

On this macOS arm64 host, native Linux x86_64 and Windows x64 artifacts require
their declared CI runners. OCI runtime execution additionally requires a
generated context with a pinned base image and a Linux/amd64 Docker/BuildKit
environment; the checked-in template deliberately returns
`INCOMPLETE/pinned_base_image_required`.

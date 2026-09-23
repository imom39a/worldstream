# Agent Swarm native distribution

Agent Swarm has a standalone local-app archive contract. It is deliberately
separate from `scripts/package.py`: the frozen WorldStream server release
inventory does not advertise macOS binaries and must not be widened implicitly.

Each application archive contains the Agent Swarm TUI, long-lived execution daemon,
daemon control client, exact process guard, controlled test worker, local
WorldStream Controller/Runtime binaries, companion helpers, and one exact
immutable Agent Swarm Activity Pack revision. Package
metadata binds every file by SHA-256 and records these update invariants:

- recovered execution starts suspended until an explicit Resume;
- provider CLIs are discovered, never installed, upgraded, logged in, or
  reconfigured by the package;
- protected application state and contribution artifacts live in the platform
  user-data directory, not beside the application binaries;
- older Pack revisions remain available for Rooms created with them.

The Room remains authoritative for shared activity and Actions. The daemon
journal owns only local operational execution state and may not manufacture
Room facts or Participant authority. Start `bin/worldstream-agent-swarmd` with
the exact absolute path to the same bundle's
`bin/worldstream-agent-swarm-process-guard`; alternate guard binaries are not a
supported packaging contract.

Inside `worldstream-agent-swarm-<application-version>-<target>/`, executables
are under `bin/`; the bundled Activity Pack is currently v0.2.0 and lives under
`packs/worldstream.agent-swarm/<semantic-digest>/`. Install
metadata is `metadata/install.json`, and payload hashes are
`checksums.sha256`. The standalone, integrity-bound WorldStream configuration
is `config/local.toml`. Runtime, application, execution, and contribution state
is operator-selected, protected, and external to this immutable directory.

Build on the native target OS and architecture. Cross-built or relabelled
archives are rejected.

```sh
scripts/package-agent-swarm.sh \
  packs/agent-swarm/releases/0.2.0/worldstream-agent-swarm-candidate.wspack \
  0.2.0 \
  blake3:EXACT_SEMANTIC_REVISION \
  dist/agent-swarm
```

On Windows x64:

```powershell
scripts/package-agent-swarm.ps1 `
  -PackPath packs/agent-swarm/releases/0.2.0/worldstream-agent-swarm-candidate.wspack `
  -PackVersion 0.2.0 `
  -PackDigest blake3:EXACT_SEMANTIC_REVISION `
  -OutputDirectory dist/agent-swarm
```

The wrappers build with the locked workspace toolchain and then invoke
`scripts/agent-swarm-package.py`. Verify an existing artifact independently:

```sh
uv run --project sdk/python --python 3.14.7 python \
  scripts/agent-swarm-package.py verify \
  dist/agent-swarm/worldstream-agent-swarm-<application-version>-macos-arm64.tar.gz \
  --target macos-arm64
```

Verify every downloaded update before extraction. Install it beside the old
version, retain protected state and all Pack revisions referenced by existing
Rooms, and restart the daemon with the new exact process guard. Recovery is
suspended until an explicit Resume; package installation never auto-resumes
work. Rollback changes binaries only and does not rewind Room facts or external
effects.

The bounded extractor verifies the archive before it writes anything and
refuses to replace an existing destination:

```sh
uv run --project sdk/python --python 3.14.7 python \
  scripts/agent-swarm-package.py extract \
  dist/agent-swarm/worldstream-agent-swarm-<application-version>-macos-arm64.tar.gz \
  --target macos-arm64 \
  --output /absolute/new/application-directory
```

Native gates point the PTY acceptance harness at the executable in this
verified extraction. Keyboard input, Unicode paste, resize/narrow rendering,
normal exit, injected backend/input errors, and terminal restoration are
therefore checked against the packaged binary on macOS and Windows.

The native package gate can perform that update flow from two independently
verified archives. It extracts immutable application roots beside one another,
starts the older daemon and managed application with external state, retains a
reviewed Room/Result/Contribution/artifact, restarts from the adjacent update,
reopens that same state without changing roster selections, verifies recovered
execution admits no queued work, and only then sends an explicit Resume:

```sh
uv run --project sdk/python --python 3.14.7 python \
  scripts/verify-agent-swarm-package.py \
  dist/agent-swarm/worldstream-agent-swarm-<new-application-version>-macos-arm64.tar.gz \
  --previous-archive \
  dist/agent-swarm/worldstream-agent-swarm-<previous-application-version>-macos-arm64.tar.gz \
  --target macos-arm64
```

The two verified manifests must name distinct application versions and the
same native target. Without `--previous-archive`, the verifier still exercises
an isolated side-by-side reinstall and reports
`reinstall_only_update_not_exercised`; it never presents that result as update
evidence. The updated binaries reopen through the exact configuration path
selected by the original initialized Controller; adjacent installation paths
are never treated as interchangeable configuration authority. Only the fixed
Runtime executable path may move to the adjacent bundle, and only after the
retained generation proves a clean stop with released ownership; all other
managed launch policy remains exact.

See [the installation and update guide](../../docs/agent-swarm-install.md) for
the user-facing flow. Native mixed-provider readiness is a separate,
fail-closed qualification described in
[the qualification guide](../../docs/agent-swarm-qualification.md).
Deterministic controlled-worker and packaging checks do not constitute real
Codex/Claude/Kiro evidence or native Windows provider qualification.

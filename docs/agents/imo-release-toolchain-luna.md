# IMO-59/60/61 Luna release toolchain handoff

Audit date: 2026-08-21

## Result

The macOS arm64 host can satisfy the declared release toolchain through the
existing nvm, uv, and Corepack installations. No compatibility declaration,
quickstart script, or packaging test was edited. The source-only macOS
quickstart passed end to end.

## Repository declarations inspected

Commands:

```sh
nl -ba compatibility.toml | sed -n '31,38p'
nl -ba package.json | sed -n '1,8p'
nl -ba .python-version
```

Results:

```text
compatibility.toml: rust = "1.97.1", node = "24.18.1", python_quickstart = "3.14.7"
package.json: packageManager = "pnpm@11.19.0", engines.node = "24.18.1"
.python-version: 3.14.7
```

The manifest also declares `node_usage = "build_only"`. `uv 0.12.5` is the
documented uv pin in `README.md`.

## Host and availability checks

Commands:

```sh
uname -srm
arch
sw_vers
df -P .
diskutil info "$(df . | awk 'NR == 2 { print $1 }')" | rg 'Type \(Bundle\)'
```

Results:

```text
Darwin 24.6.0 arm64
arm64
ProductName: macOS
ProductVersion: 15.6.1
BuildVersion: 24G90
Type (Bundle): apfs
```

The repository is on APFS, satisfying the quickstart preflight.

Initial ambient-shell probes showed:

```text
node: v25.9.0
pnpm: command not found
python3: pyenv: version `3.14.7' is not installed (set by .python-version)
uv: uv 0.12.5 (Homebrew 2026-08-14 aarch64-apple-darwin)
corepack: 0.29.4
```

The Homebrew Corepack probe was fail-closed:

```sh
corepack pnpm --version
```

```text
Error: Cannot find matching keyid: {"signatures":[{"keyid":"SHA256:DhQ8wR5APBvFHLF/+Tc+AYvPOdTpcIDqOhxsBHRwC7U"}],"keys":[{"keyid":"SHA256:jl3bwswu80PjjokCgh0o2w5c2U4LhQAE57gj9cz1kzA"}]}
```

These ambient defaults are not the pinned toolchain path.

## Existing-tool provisioning checks

Commands:

```sh
zsh -lic 'nvm ls 24.18.1'
zsh -lic 'nvm exec 24.18.1 node --version'
zsh -lic 'nvm exec 24.18.1 corepack --version'
zsh -lic 'nvm exec 24.18.1 corepack pnpm --version'
zsh -lic 'nvm exec 24.18.1 corepack enable'
zsh -lic 'nvm exec 24.18.1 pnpm --version'
uv python list
uv python find 3.14.7
uv run --project sdk/python --locked --python 3.14.7 python --version
```

Results:

```text
nvm: v24.18.1 installed
nvm Node: v24.18.1
nvm Corepack: 0.35.0
nvm Corepack pnpm: 11.19.0
after corepack enable: pnpm 11.19.0
uv: 0.12.5
uv Python: /Users/vinothshanmugam/.local/share/uv/python/cpython-3.14.7-macos-aarch64-none/bin/python3.14
Python 3.14.7
```

The exact pins were also checked in one shell after selecting nvm Node and
putting the uv Python directory first on `PATH`:

```text
node: v24.18.1
pnpm: 11.19.0
python3: Python 3.14.7
uv: uv 0.12.5 (Homebrew 2026-08-14 aarch64-apple-darwin)
cargo: cargo 1.97.1 (c980f4866 2026-06-30)
rustc: rustc 1.97.1 (8bab26f4f 2026-07-14)
```

`uv run` provisioned/recreated the ignored `sdk/python/.venv`; no tracked
files were changed by that operation.

## Quickstart verification

Exact command:

```sh
zsh -lic '
  nvm use --silent 24.18.1 >/dev/null
  uv_python="$(uv python find 3.14.7)"
  uv_python_dir="$(dirname "$uv_python")"
  export PATH="$uv_python_dir:$PATH"
  ./scripts/macos-source-quickstart.sh
'
```

Result: exit status `0`.

Relevant output:

```text
macOS source quickstart: APFS, Python 3.14.7, Node/pnpm lockfile checks
Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.29s
52 passed in 32.97s
Done in 285ms using pnpm v11.19.0
Test Files  3 passed (3)
Tests  23 passed (23)
vite v8.2.1 building client environment for production...
✓ built in 125ms
compatibility manifest verified (specification-only, release_ready=false)
macOS source quickstart checks passed; no signed or notarized binary was produced.
```

The quickstart therefore passed cargo build, locked Python sync/tests, frozen
pnpm install, UI tests/build, and manifest verification.

Audit note: the quickstart's own preflight checks command presence and the
`.python-version` shape; the exact Node, pnpm, Python, and uv versions above
were proven by the pinned-shell probes. No script change was justified because
the pinned existing-tool path succeeds and no behavioral defect was observed.

## External blockers and boundaries

- There is no host-toolchain blocker when the pinned nvm Node/Corepack and uv
  Python paths are selected.
- An unpinned default shell is unsuitable: it selects Node `25.9.0`, a pyenv
  shim without Python `3.14.7`, and Homebrew Corepack `0.29.4` with the keyid
  mismatch above. CI or operators must select the pinned paths explicitly.
- The pre-existing declarations still say `release_ready = false` and
  `release_status = "implementation_in_progress_release_evidence_incomplete"`.
  The `macos-source-quickstart` evidence entry has an empty
  `artifact_digest`, as do other required evidence fields. This lane does not
  invent or write those digests.
- macOS remains source-only; this run produced no binary, signature,
  notarization, SBOM, or provenance claim.

## Files changed

```text
docs/agents/imo-release-toolchain-luna.md  (added)
```

`compatibility.toml`, `compatibility.json`, and
`scripts/macos-source-quickstart.sh` were not edited. Linear statuses were not
changed.

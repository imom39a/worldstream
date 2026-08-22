#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python_bin="${WORLDSTREAM_RELEASE_PYTHON:-}"
if [[ -z "$python_bin" ]]; then
    python_bin="$(command -v python3 2>/dev/null || true)"
fi
if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then
    echo "OCI packaging requires an executable WORLDSTREAM_RELEASE_PYTHON" >&2
    exit 2
fi
expected_python="$(tr -d '[:space:]' < "$workspace_dir/.python-version")"
actual_python="$("$python_bin" -I -c 'import platform; print(platform.python_version())')"
if [[ "$actual_python" != "$expected_python" ]]; then
    echo "OCI packaging requires Python $expected_python, observed $actual_python" >&2
    exit 2
fi

# package.py owns generation, context verification, and the canonical
# content-only inventory. Keeping this wrapper as an interpreter selector
# prevents a second shell-local verifier from drifting from the implementation
# used by PowerShell and direct callers.
exec "$python_bin" -I "$workspace_dir/scripts/package.py" oci-context "$@"

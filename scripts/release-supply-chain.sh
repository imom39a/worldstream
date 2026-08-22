#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
workspace_dir=$(cd -- "$script_dir/.." && pwd)
python_bin="${WORLDSTREAM_RELEASE_PYTHON:-}"
if [[ -z "$python_bin" ]]; then
    python_bin="$(command -v python3 2>/dev/null || true)"
fi
if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then
    echo "release supply-chain generation requires an executable WORLDSTREAM_RELEASE_PYTHON" >&2
    exit 2
fi
expected_python="$(tr -d '[:space:]' < "$workspace_dir/.python-version")"
actual_python="$("$python_bin" -I -c 'import platform; print(platform.python_version())')"
if [[ "$actual_python" != "$expected_python" ]]; then
    echo "release supply-chain generation requires Python $expected_python, observed $actual_python" >&2
    exit 2
fi
exec "$python_bin" -I "$script_dir/release-supply-chain.py" "$@"

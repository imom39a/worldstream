#!/usr/bin/env bash
set -Eeuo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
daemon_bin="${WORLDSTREAMD_BIN:-$workspace_dir/target/debug/worldstreamd}"

if ! command -v uv >/dev/null 2>&1; then
  printf '%s\n' 'daemon transition soak: uv is required for the locked public SDK environment' >&2
  exit 2
fi
exec uv run --project "$workspace_dir/sdk/python" --locked \
  python "$workspace_dir/scripts/daemon-transition-soak.py" \
  --daemon-bin "$daemon_bin" "$@"

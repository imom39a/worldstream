#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# package.py owns generation, context verification, and the canonical
# content-only inventory. Keeping this wrapper as an interpreter selector
# prevents a second shell-local verifier from drifting from the implementation
# used by PowerShell and direct callers.
exec python3 "$workspace_dir/scripts/package.py" oci-context "$@"

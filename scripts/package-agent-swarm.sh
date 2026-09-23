#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  printf '%s\n' 'Agent Swarm macOS archives must be built on native macOS.' >&2
  exit 2
fi
if [[ $# -ne 4 ]]; then
  printf 'usage: %s PACK_PATH PACK_VERSION PACK_DIGEST OUTPUT_DIR\n' "$0" >&2
  exit 2
fi

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
pack_path="$1"
pack_version="$2"
pack_digest="$3"
output_dir="$4"
case "$(uname -m)" in
  arm64) target=macos-arm64 ;;
  x86_64) target=macos-x86_64 ;;
  *) printf 'unsupported macOS architecture: %s\n' "$(uname -m)" >&2; exit 2 ;;
esac

cd "$workspace_dir"
cargo build --release --locked -p worldstream-server -p worldstream-studio-supervisor --bins
cargo build --release --locked -p worldstream-agent-swarm --features managed-local-runtime --bins
version="$(uv run --project sdk/python --python 3.14.7 python -c 'import tomllib; print(tomllib.load(open("compatibility.toml", "rb"))["contracts"]["product"])')"
uv run --project sdk/python --python 3.14.7 python scripts/agent-swarm-package.py build \
  --target "$target" \
  --binary-dir target/release \
  --pack "$pack_path" \
  --pack-version "$pack_version" \
  --pack-digest "$pack_digest" \
  --version "$version" \
  --output "$output_dir"

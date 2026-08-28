#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

cargo build -p worldstream-server --bin worldstreamd
cargo run -p worldstream-studio-supervisor --bin worldstream-studio-supervisor -- "$@" &
supervisor_pid=$!
cleanup() {
  kill "$supervisor_pid" 2>/dev/null || true
  wait "$supervisor_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

pnpm --dir web/studio dev

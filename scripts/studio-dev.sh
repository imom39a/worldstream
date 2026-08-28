#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

cargo build --locked -p worldstream-server --bin worldstreamd
cargo build --locked -p worldstream-studio-supervisor --bins
target/debug/worldstream-studio-supervisor "$@" &
supervisor_pid=$!
cleanup() {
  kill "$supervisor_pid" 2>/dev/null || true
  wait "$supervisor_pid" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

for _ in {1..20}; do
  if ! kill -0 "$supervisor_pid" 2>/dev/null; then
    wait "$supervisor_pid"
    exit 1
  fi
  sleep 0.05
done

pnpm --dir web/studio dev

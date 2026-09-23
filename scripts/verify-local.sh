#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

required_rust="1.97.1"
required_node="24.18.1"
required_python="3.14.7"
required_pnpm="11.19.0"

for tool in rustc cargo node pnpm uv python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    printf 'missing required tool: %s\n' "$tool" >&2
    exit 1
  fi
done

actual_rust="$(rustc --version | awk '{print $2}')"
actual_node="$(node -p 'process.versions.node')"
actual_pnpm="$(pnpm --version)"

if [[ "$actual_rust" != "$required_rust" ]]; then
  printf 'Rust mismatch: need %s, found %s\n' "$required_rust" "$actual_rust" >&2
  exit 1
fi
if [[ "$actual_node" != "$required_node" ]]; then
  printf 'Node mismatch: need %s, found %s\n' "$required_node" "$actual_node" >&2
  exit 1
fi
if [[ "$actual_pnpm" != "$required_pnpm" ]]; then
  printf 'pnpm mismatch: need %s, found %s\n' "$required_pnpm" "$actual_pnpm" >&2
  exit 1
fi

cargo_offline=()
uv_offline=()
pnpm_offline=()
if [[ "${WORLDSTREAM_VERIFY_OFFLINE:-0}" == "1" ]]; then
  cargo_offline=(--offline)
  uv_offline=(--offline)
  pnpm_offline=(--offline)
fi

uv sync "${uv_offline[@]}" --project sdk/python --locked --python "$required_python"
actual_python="$(uv run "${uv_offline[@]}" --project sdk/python --python "$required_python" python -c 'import platform; print(platform.python_version())')"
if [[ "$actual_python" != "$required_python" ]]; then
  printf 'Python mismatch: need %s, found %s\n' "$required_python" "$actual_python" >&2
  exit 1
fi

python3 scripts/verify-manifest.py
cargo fmt --all -- --check
cargo clippy "${cargo_offline[@]}" --workspace --all-targets --locked -- -D warnings
cargo build "${cargo_offline[@]}" --workspace --locked
cargo test "${cargo_offline[@]}" --workspace --locked
cargo run "${cargo_offline[@]}" --locked -p xtask -- compat verify

uv run "${uv_offline[@]}" --project sdk/python --python "$required_python" ruff format --check
uv run "${uv_offline[@]}" --project sdk/python --python "$required_python" ruff check
uv run "${uv_offline[@]}" --project sdk/python --python "$required_python" pytest --ignore=target

pnpm install "${pnpm_offline[@]}" --frozen-lockfile
pnpm --dir sdk/typescript-client lint
pnpm --dir sdk/typescript-client test
pnpm --dir clients/agent-heist-web lint
pnpm --dir clients/agent-heist-web test
pnpm --dir clients/agent-heist-web build
pnpm --dir web/console lint
pnpm --dir web/console test
pnpm --dir web/console build

scripts/smoke-operator.sh

if [[ "$(uname -s)" == "Darwin" ]]; then
  scripts/verify-agent-swarm-native.sh
fi

printf '%s\n' 'WorldStream bootstrap verification passed.'

#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
offline=()
if [[ "${WORLDSTREAM_VERIFY_OFFLINE:-0}" == "1" ]]; then
  offline=(--offline)
fi
cargo fmt --all -- --check
cargo check "${offline[@]}" --locked --all-targets
cargo build "${offline[@]}" --locked
cargo test "${offline[@]}" --locked -- --test-threads=1
cargo run "${offline[@]}" --locked -p xtask -- compat verify
printf '%s\n' 'Kernel and operator verification passed.'

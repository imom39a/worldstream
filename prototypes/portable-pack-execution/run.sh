#!/usr/bin/env bash
set -euo pipefail

prototype_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$prototype_dir"

mkdir -p artifacts

npx --yes --package typescript@5.9.2 \
  tsc --noEmit --strict --target ES2022 --module ESNext \
  guests/typescript/pack.ts

npx --yes @bytecodealliance/jco@1.32.1 componentize \
  guests/typescript/pack.ts \
  --wit wit/worldstream-pack.wit \
  --world-name activity-pack \
  --disable all \
  -o artifacts/typescript-pack.component.wasm

cargo run \
  --manifest-path host/Cargo.toml \
  --release \
  -- \
  artifacts/typescript-pack.component.wasm \
  artifacts/PROTOTYPE-pack-store


#!/bin/sh
# Exact cc-rs driver for the byte-pinned Zig x86_64 Linux archive.

set -eu

case "${WORLDSTREAM_ZIG:-}" in
  /*) ;;
  *)
    echo "WORLDSTREAM_ZIG must name the absolute pinned Zig executable" >&2
    exit 2
    ;;
esac

if [ -L "$WORLDSTREAM_ZIG" ] || [ ! -f "$WORLDSTREAM_ZIG" ] || [ ! -x "$WORLDSTREAM_ZIG" ]; then
  echo "WORLDSTREAM_ZIG is not an exact regular executable" >&2
  exit 2
fi

exec "$WORLDSTREAM_ZIG" cc -target x86_64-linux-musl "$@"

#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

report_path=""
while (($#)); do
  case "$1" in
    --report)
      (($# >= 2)) || { printf '%s\n' '--report requires a path' >&2; exit 2; }
      report_path="$2"
      shift 2
      ;;
    --help|-h)
      printf '%s\n' 'Usage: scripts/macos-source-quickstart.sh [--report PATH]'
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  printf '%s\n' 'macOS source quickstart must run on Darwin; no binary packaging is attempted.' >&2
  exit 1
fi

if ! command -v sw_vers >/dev/null 2>&1; then
  printf '%s\n' 'macOS source quickstart cannot verify the required macOS 15+ version.' >&2
  exit 1
fi
macos_version="$(sw_vers -productVersion 2>/dev/null || true)"
macos_major="${macos_version%%.*}"
if [[ ! "$macos_version" =~ ^[0-9]+\.[0-9]+([.][0-9]+)?$ ]] \
  || [[ ! "$macos_major" =~ ^[0-9]+$ ]] \
  || ((macos_major < 15)); then
  printf 'macOS source quickstart requires macOS 15+; found %s\n' \
    "${macos_version:-unknown}" >&2
  exit 1
fi

machine="$(uname -m)"
case "$machine" in
  arm64|x86_64) ;;
  *)
    printf 'macOS source quickstart requires arm64 or x86_64; found %s\n' \
      "${machine:-unknown}" >&2
    exit 1
    ;;
esac

device="$(df "$workspace_dir" | awk 'NR == 2 { print $1 }')"
filesystem=""
if [[ -n "$device" ]] && command -v diskutil >/dev/null 2>&1; then
  filesystem="$(diskutil info "$device" 2>/dev/null \
    | awk -F: '/Type \(Bundle\)/ { gsub(/[[:space:]]/, "", $2); print tolower($2); exit }')"
fi
if [[ "$filesystem" != "apfs" ]]; then
  printf 'macOS source quickstart requires APFS; found %s\n' "${filesystem:-unknown}" >&2
  exit 1
fi

missing_commands=()
for command_name in cargo git node python3 pnpm rustc uv; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    missing_commands+=("$command_name")
  fi
done
if ((${#missing_commands[@]})); then
  printf 'macOS source quickstart is missing required commands: %s\n' \
    "${missing_commands[*]}" >&2
  printf '%s\n' 'Install the pinned toolchain before retrying; no build was attempted.' >&2
  exit 1
fi

source_revision="$(git rev-parse --verify HEAD^{commit} 2>/dev/null || true)"
if [[ ! "$source_revision" =~ ^[0-9a-f]{40}$ ]]; then
  printf 'macOS source quickstart cannot identify the exact source revision: %s\n' \
    "${source_revision:-unknown}" >&2
  exit 1
fi

python_version="$(tr -d '[:space:]' < .python-version)"
if [[ ! "$python_version" =~ ^3\.[0-9]+\.[0-9]+$ ]]; then
  printf 'macOS source quickstart has an invalid .python-version: %s\n' "$python_version" >&2
  exit 1
fi

node_version="$(tr -d '[:space:]' < .node-version)"
if [[ ! "$node_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'macOS source quickstart has an invalid .node-version: %s\n' "$node_version" >&2
  exit 1
fi
actual_node_version="$(node --version)"
actual_node_version="${actual_node_version#v}"
if [[ "$actual_node_version" != "$node_version" ]]; then
  printf 'macOS source quickstart requires Node %s; found %s\n' \
    "$node_version" "$actual_node_version" >&2
  exit 1
fi

package_manager="$(node -p "require('./package.json').packageManager" 2>/dev/null || true)"
if [[ "$package_manager" != pnpm@* ]]; then
  printf 'macOS source quickstart requires a pinned pnpm packageManager\n' >&2
  exit 1
fi
required_pnpm_version="${package_manager#pnpm@}"
actual_pnpm_version="$(pnpm --version)"
if [[ "$actual_pnpm_version" != "$required_pnpm_version" ]]; then
  printf 'macOS source quickstart requires pnpm %s; found %s\n' \
    "$required_pnpm_version" "$actual_pnpm_version" >&2
  exit 1
fi

uv_version="$(tr -d '[:space:]' < .uv-version)"
if [[ ! "$uv_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  printf 'macOS source quickstart has an invalid .uv-version: %s\n' "$uv_version" >&2
  exit 1
fi
actual_uv_version="$(uv --version | awk '{print $2}')"
if [[ "$actual_uv_version" != "$uv_version" ]]; then
  printf 'macOS source quickstart requires uv %s; found %s\n' \
    "$uv_version" "$actual_uv_version" >&2
  exit 1
fi

rust_version="$(sed -n 's/^channel = "\([^"]*\)"/\1/p' rust-toolchain.toml)"
actual_rust_version="$(rustc --version | awk '{print $2}')"
if [[ -z "$rust_version" || "$actual_rust_version" != "$rust_version" ]]; then
  printf 'macOS source quickstart requires Rust %s; found %s\n' \
    "${rust_version:-unknown}" "$actual_rust_version" >&2
  exit 1
fi

printf 'macOS source quickstart: APFS, Python %s, Node/pnpm lockfile checks\n' \
  "$python_version"
uv sync --project sdk/python --locked --python "$python_version"
selected_python_version="$(uv run --project sdk/python --locked python --version \
  | awk '{print $2}')"
if [[ "$selected_python_version" != "$python_version" ]]; then
  printf 'macOS source quickstart requires Python %s; found %s\n' \
    "$python_version" "$selected_python_version" >&2
  exit 1
fi
WORLDSTREAM_BUILD_REVISION="$source_revision" cargo build --workspace --locked
uv run --project sdk/python --locked pytest -q
pnpm install --frozen-lockfile
pnpm --dir web/console test
pnpm --dir web/console build
uv run --project sdk/python --locked python scripts/verify-manifest.py

quickstart_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-macos-quickstart.XXXXXX")"
chmod 700 "$quickstart_root"
quickstart_pid=""
cleanup() {
  if [[ -n "$quickstart_pid" ]] && kill -0 "$quickstart_pid" 2>/dev/null; then
    kill -TERM "$quickstart_pid" 2>/dev/null || true
    wait "$quickstart_pid" 2>/dev/null || true
  fi
  if [[ -n "$quickstart_root" \
    && "$quickstart_root" == *"/worldstream-macos-quickstart."* \
    && -d "$quickstart_root" ]]; then
    rm -rf -- "$quickstart_root"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

mkdir -m 700 "$quickstart_root/data"
uv run --project sdk/python --locked python - "$quickstart_root/authority.secret" <<'PY'
import secrets
import sys
from pathlib import Path

Path(sys.argv[1]).write_bytes(secrets.token_bytes(32))
PY
chmod 600 "$quickstart_root/authority.secret"

quickstart_port="$(uv run --project sdk/python --locked python - <<'PY'
import socket

with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
)"
product_version="$(uv run --project sdk/python --locked python - <<'PY'
import tomllib
from pathlib import Path

manifest = tomllib.loads(Path("compatibility.toml").read_text(encoding="utf-8"))
print(manifest["contracts"]["product"])
PY
)"
WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE="$quickstart_root/authority.secret" \
  target/debug/worldstreamd \
    --storage-profile sqlite-bundled \
    --data-dir "$quickstart_root/data" \
    --bind "127.0.0.1:$quickstart_port" \
    >"$quickstart_root/worldstreamd.log" 2>&1 &
quickstart_pid="$!"

probe_passed=0
for ((attempt = 0; attempt < 50; attempt += 1)); do
  if ! kill -0 "$quickstart_pid" 2>/dev/null; then
    break
  fi
  if uv run --project sdk/python --locked python scripts/package.py probe \
    --base-url "http://127.0.0.1:$quickstart_port" \
    --version "$product_version" \
    --storage-profile sqlite-bundled \
    --timeout 1 >"$quickstart_root/probe.log" 2>&1; then
    probe_passed=1
    break
  fi
  sleep 0.1
done
if ((probe_passed == 0)); then
  printf '%s\n' 'macOS source quickstart runtime probe failed closed.' >&2
  sed -n '1,120p' "$quickstart_root/probe.log" >&2 2>/dev/null || true
  sed -n '1,120p' "$quickstart_root/worldstreamd.log" >&2 2>/dev/null || true
  exit 1
fi
cat "$quickstart_root/probe.log"
kill -TERM "$quickstart_pid"
set +e
wait "$quickstart_pid"
shutdown_status="$?"
set -e
if ((shutdown_status != 0 && shutdown_status != 143)); then
  printf 'macOS source quickstart daemon stop returned unexpected status %s.\n' \
    "$shutdown_status" >&2
  exit 1
fi
quickstart_pid=""

if [[ -n "$report_path" ]]; then
  if [[ -L "$report_path" || -d "$report_path" ]]; then
    printf 'macOS source quickstart report path is unsafe: %s\n' "$report_path" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$report_path")"
  uv run --project sdk/python --locked python - \
    "$report_path" "$product_version" "$macos_version" "$filesystem" \
    "$machine" "$rust_version" "$python_version" "$node_version" \
    "$required_pnpm_version" "$uv_version" "$source_revision" <<'PY'
import json
import os
import sys
import tempfile
from pathlib import Path

destination = Path(sys.argv[1])
report = {
    "schema": "worldstream/macos-source-quickstart/v1",
    "status": "passed",
    "release_evidence": False,
    "signed_or_notarized_binary": False,
    "version": sys.argv[2],
    "platform": {
        "system": "Darwin",
        "version": sys.argv[3],
        "filesystem": sys.argv[4],
        "machine": sys.argv[5],
    },
    "toolchains": {
        "rust": sys.argv[6],
        "python": sys.argv[7],
        "node": sys.argv[8],
        "pnpm": sys.argv[9],
        "uv": sys.argv[10],
    },
    "source_revision": sys.argv[11],
    "checks": {
        "pinned_toolchain": True,
        "source_revision": True,
        "source_build": True,
        "quickstart": True,
    },
}
descriptor, temporary_name = tempfile.mkstemp(
    prefix=f".{destination.name}.", dir=destination.parent
)
temporary = Path(temporary_name)
try:
    with os.fdopen(descriptor, "w", encoding="utf-8") as output:
        json.dump(report, output, indent=2, sort_keys=True)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.chmod(temporary, 0o644)
    os.replace(temporary, destination)
except BaseException:
    temporary.unlink(missing_ok=True)
    raise
PY
fi

printf '%s\n' 'macOS source quickstart build and runtime checks passed; no signed or notarized binary was produced.'

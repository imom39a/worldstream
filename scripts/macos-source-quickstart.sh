#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

report_path=""
browser_path=""
browser_version=""
browser_sha256=""
browser_size_bytes=""
browser_archive_url=""
browser_archive_sha256=""
browser_archive_size_bytes=""
quickstart_started_seconds="$SECONDS"
while (($#)); do
  case "$1" in
    --report)
      (($# >= 2)) || { printf '%s\n' '--report requires a path' >&2; exit 2; }
      report_path="$2"
      shift 2
      ;;
    --browser) browser_path="$2"; shift 2 ;;
    --browser-version) browser_version="$2"; shift 2 ;;
    --browser-sha256) browser_sha256="$2"; shift 2 ;;
    --browser-size-bytes) browser_size_bytes="$2"; shift 2 ;;
    --browser-archive-url) browser_archive_url="$2"; shift 2 ;;
    --browser-archive-sha256) browser_archive_sha256="$2"; shift 2 ;;
    --browser-archive-size-bytes) browser_archive_size_bytes="$2"; shift 2 ;;
    --help|-h)
      printf '%s\n' 'Usage: scripts/macos-source-quickstart.sh --browser PATH --browser-version VERSION --browser-sha256 HEX --browser-size-bytes N --browser-archive-url URL --browser-archive-sha256 HEX --browser-archive-size-bytes N [--report PATH]'
      exit 0
      ;;
    *)
      printf 'unknown argument: %s\n' "$1" >&2
      exit 2
      ;;
  esac
done

if [[ -z "$browser_path" || ! -x "$browser_path" \
  || ! "$browser_version" =~ ^[0-9]+([.][0-9]+){3}$ \
  || ! "$browser_sha256" =~ ^[0-9a-f]{64}$ \
  || ! "$browser_size_bytes" =~ ^[1-9][0-9]*$ \
  || ! "$browser_archive_url" =~ ^https://storage[.]googleapis[.]com/chrome-for-testing-public/ \
  || ! "$browser_archive_sha256" =~ ^[0-9a-f]{64}$ \
  || ! "$browser_archive_size_bytes" =~ ^[1-9][0-9]*$ ]]; then
  printf '%s\n' 'macOS source quickstart requires a complete pinned browser identity.' >&2
  exit 2
fi

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
  arm64)
    expected_browser_url="https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-arm64/chrome-headless-shell-mac-arm64.zip"
    expected_browser_archive_sha256="ef5d61434f13d9d2d9bdc7c9ab4bff92225979e196458cf846640862b25f127d"
    expected_browser_archive_size=98034515
    expected_browser_sha256="4e0c165ef2f0d7265fb1e6b3df2d03d1d6581fb72cdfcebeac19c09760571df6"
    expected_browser_size=167333040
    ;;
  x86_64)
    expected_browser_url="https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-x64/chrome-headless-shell-mac-x64.zip"
    expected_browser_archive_sha256="a50c716727adf9e4af5b8861d19da23b672b4687a0d631ec4beebb3009d6db97"
    expected_browser_archive_size=102893170
    expected_browser_sha256="49b6e6bdc4a9a14a160a2fcb08576ec3acef1d360cfa61bdc79a400a27a16d99"
    expected_browser_size=182459908
    ;;
  *)
    printf 'macOS source quickstart requires arm64 or x86_64; found %s\n' \
      "${machine:-unknown}" >&2
    exit 1
    ;;
esac
if [[ "$browser_version" != "152.0.7977.54" \
  || "$browser_archive_url" != "$expected_browser_url" \
  || "$browser_archive_sha256" != "$expected_browser_archive_sha256" \
  || "$browser_archive_size_bytes" != "$expected_browser_archive_size" \
  || "$browser_sha256" != "$expected_browser_sha256" \
  || "$browser_size_bytes" != "$expected_browser_size" ]]; then
  printf '%s\n' 'macOS source quickstart browser identity does not match the frozen architecture pin.' >&2
  exit 1
fi

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
cleanup() {
  if [[ -n "$quickstart_root" \
    && "$quickstart_root" == *"/worldstream-macos-quickstart."* \
    && -d "$quickstart_root" ]]; then
    rm -rf -- "$quickstart_root"
  fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM HUP

product_version="$(uv run --project sdk/python --locked python - <<'PY'
import tomllib
from pathlib import Path

manifest = tomllib.loads(Path("compatibility.toml").read_text(encoding="utf-8"))
print(manifest["contracts"]["product"])
PY
)"
mkdir -m 700 "$quickstart_root/cdp-state"
WORLDSTREAM_PYTHON="$(pwd)/sdk/python/.venv/bin/python" \
WORLDSTREAM_BROWSER_MODE=cdp \
WORLDSTREAM_BROWSER_PACKAGE_MODE=0 \
WORLDSTREAM_BROWSER_PREBUILT_UI=1 \
WORLDSTREAM_BROWSER_WORLDSTREAMD="$(pwd)/target/debug/worldstreamd" \
WORLDSTREAM_BROWSER_UI_DIR="$(pwd)/web/console/dist" \
WORLDSTREAM_BROWSER_SDK_SRC="$(pwd)/sdk/python/src" \
WORLDSTREAM_BROWSER_HEIST_DIR="$(pwd)/examples/heist" \
WORLDSTREAM_BROWSER_REPORT="$quickstart_root/browser-story.json" \
WORLDSTREAM_BROWSER_STORY_TIMEOUT=240 \
WORLDSTREAM_CDP_ADAPTER="$(pwd)/scripts/cdp-browser.py" \
WORLDSTREAM_CDP_STATE_DIR="$quickstart_root/cdp-state" \
WORLDSTREAM_BROWSER_BINARY="$browser_path" \
WORLDSTREAM_BROWSER_VERSION="$browser_version" \
WORLDSTREAM_BROWSER_SHA256="$browser_sha256" \
WORLDSTREAM_BROWSER_SIZE_BYTES="$browser_size_bytes" \
WORLDSTREAM_BROWSER_ARCHIVE_URL="$browser_archive_url" \
WORLDSTREAM_BROWSER_ARCHIVE_SHA256="$browser_archive_sha256" \
WORLDSTREAM_BROWSER_ARCHIVE_SIZE_BYTES="$browser_archive_size_bytes" \
  web/console/live-browser-story.sh >"$quickstart_root/browser-story.log" 2>&1 || {
    sed -n '1,160p' "$quickstart_root/browser-story.log" >&2 || true
    printf '%s\n' 'macOS source quickstart complete Heist browser story failed closed.' >&2
    exit 1
  }
quickstart_elapsed_seconds="$((SECONDS - quickstart_started_seconds))"
if ((quickstart_elapsed_seconds <= 0 || quickstart_elapsed_seconds >= 600)); then
  printf 'macOS source quickstart exceeded the frozen <10m contract: %ss\n' \
    "$quickstart_elapsed_seconds" >&2
  exit 1
fi

if [[ -n "$report_path" ]]; then
  if [[ -L "$report_path" || -d "$report_path" ]]; then
    printf 'macOS source quickstart report path is unsafe: %s\n' "$report_path" >&2
    exit 1
  fi
  mkdir -p "$(dirname "$report_path")"
  uv run --project sdk/python --locked python - \
    "$report_path" "$product_version" "$macos_version" "$filesystem" \
    "$machine" "$rust_version" "$python_version" "$node_version" \
    "$required_pnpm_version" "$uv_version" "$source_revision" \
    "$quickstart_root/browser-story.json" "$quickstart_elapsed_seconds" <<'PY'
import json
import os
import stat
import sys
import tempfile
from pathlib import Path

destination = Path(sys.argv[1])
def reject_pairs(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate JSON key")
        value[key] = item
    return value


browser_story_path = Path(sys.argv[12])
maximum_browser_story_bytes = 8 * 1024 * 1024
before = browser_story_path.lstat()
if (
    stat.S_ISLNK(before.st_mode)
    or not stat.S_ISREG(before.st_mode)
    or before.st_size <= 0
    or before.st_size > maximum_browser_story_bytes
):
    raise ValueError("browser story input is unsafe or oversized")
flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
descriptor = os.open(browser_story_path, flags)
with os.fdopen(descriptor, "rb") as source:
    opened = os.fstat(source.fileno())
    browser_story_bytes = source.read(maximum_browser_story_bytes + 1)
    after = os.fstat(source.fileno())
identity = (before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns)
if (
    not stat.S_ISREG(opened.st_mode)
    or identity != (opened.st_dev, opened.st_ino, opened.st_size, opened.st_mtime_ns)
    or identity != (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns)
    or len(browser_story_bytes) != before.st_size
    or len(browser_story_bytes) > maximum_browser_story_bytes
):
    raise ValueError("browser story input changed during bounded read")
browser_story = json.loads(
    browser_story_bytes.decode("utf-8"),
    object_pairs_hook=reject_pairs,
    parse_constant=lambda value: (_ for _ in ()).throw(ValueError(value)),
)
if not isinstance(browser_story, dict):
    raise ValueError("browser story must be an object")
checks = browser_story.get("checks")
if not (
    browser_story.get("schema") == "worldstream/package-browser-heist/v1"
    and browser_story.get("status") == "pass"
    and browser_story.get("release_evidence") is False
    and browser_story.get("source_mode") == "source-build"
    and isinstance(checks, dict)
    and checks.get("package_bound_runtime") is False
    and checks.get("package_bound_reference_clients") is False
    and all(value is True for key, value in checks.items() if not key.startswith("package_bound_"))
):
    raise ValueError("complete macOS browser story is invalid")
report = {
    "schema": "worldstream/macos-source-quickstart/v2",
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
    "elapsed_seconds": int(sys.argv[13]),
    "browser_story": browser_story,
    "checks": {
        "complete_heist": True,
        "embedded_ui": True,
        "pinned_toolchain": True,
        "privacy": True,
        "real_browser": True,
        "replay": True,
        "source_revision": True,
        "source_build": True,
        "stale_resync": True,
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

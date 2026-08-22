#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
report_path=""
package_args=()
while (($#)); do
    case "$1" in
        --report)
            (($# >= 2)) || { echo "missing value for --report" >&2; exit 2; }
            report_path="$2"
            shift 2
            ;;
        --report=*)
            report_path="${1#*=}"
            shift
            ;;
        *)
            package_args+=("$1")
            shift
            ;;
    esac
done
python3 "$workspace_dir/scripts/package.py" package "${package_args[@]}"

# package.py owns archive verification and the canonical report schema.  The
# wrapper only locates the exact archive selected by the package arguments.
python3 - "$workspace_dir" "$report_path" "${package_args[@]}" <<'PY'
from __future__ import annotations

import subprocess
import sys
import tomllib
from pathlib import Path


workspace = Path(sys.argv[1]).resolve()
report_value = sys.argv[2]
tokens = sys.argv[3:]


def option(name: str, default: str | None = None) -> str | None:
    value = default
    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token == name:
            if index + 1 >= len(tokens):
                raise SystemExit(f"missing value for {name}")
            value = tokens[index + 1]
            index += 2
            continue
        prefix = name + "="
        if token.startswith(prefix):
            value = token[len(prefix) :]
        index += 1
    return value


if "--dry-run" in tokens:
    raise SystemExit(0)

target = option("--target")
output_value = option("--output", str(workspace / "dist"))
names = {
    "source": "worldstream-{version}-source.tar.gz",
    "linux-x86_64": "worldstream-{version}-linux-x86_64.tar.gz",
    "windows-x64": "worldstream-{version}-windows-x64.zip",
}
if target not in names:
    raise SystemExit(f"unsupported or missing package target: {target!r}")
if output_value is None or not output_value:
    raise SystemExit("package output path must not be empty")

try:
    manifest = tomllib.loads(
        (workspace / "compatibility.toml").read_text(encoding="utf-8")
    )
    version = manifest["contracts"]["product"]
except (OSError, KeyError, TypeError, tomllib.TOMLDecodeError) as error:
    raise SystemExit(f"cannot read packaged product version: {error}") from error

output = Path(output_value)
if output.is_symlink() or not output.is_dir():
    raise SystemExit(f"package output is not a real directory: {output}")
artifact = output / names[target].format(version=version)
if artifact.is_symlink() or not artifact.is_file():
    raise SystemExit(f"packaging did not produce the expected regular archive: {artifact}")

report_args = [sys.executable, str(workspace / "scripts/package.py"), "report", str(artifact)]
if report_value:
    report_args.extend(("--report", report_value))
subprocess.run(report_args, check=True)
PY

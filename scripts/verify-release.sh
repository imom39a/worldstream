#!/usr/bin/env bash
set -euo pipefail

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python_bin="${WORLDSTREAM_RELEASE_PYTHON:-}"
if [[ -z "$python_bin" ]]; then
    python_bin="$(command -v python3 2>/dev/null || true)"
fi
if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then
    echo "release verification requires an executable WORLDSTREAM_RELEASE_PYTHON" >&2
    exit 2
fi
expected_python="$(tr -d '[:space:]' < "$workspace_dir/.python-version")"
actual_python="$("$python_bin" -I -c 'import platform; print(platform.python_version())')"
if [[ "$actual_python" != "$expected_python" ]]; then
    echo "release verification requires Python $expected_python, observed $actual_python" >&2
    exit 2
fi
report_path=""
release_inventory=""
verify_args=()
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
        --release-inventory)
            (($# >= 2)) || { echo "missing value for --release-inventory" >&2; exit 2; }
            release_inventory="$2"
            shift 2
            ;;
        --release-inventory=*)
            release_inventory="${1#*=}"
            shift
            ;;
        *)
            verify_args+=("$1")
            shift
            ;;
    esac
done
if [[ -n "$release_inventory" ]]; then
    verify_args+=(--release-inventory "$release_inventory")
fi
artifact_arg=""
for ((index = 0; index < ${#verify_args[@]}; index++)); do
    token="${verify_args[$index]}"
    if [[ "$token" == "--release-inventory" ]]; then
        ((index += 1))
        continue
    fi
    if [[ "$token" != -* ]]; then
        artifact_arg="$token"
        break
    fi
done
if [[ -n "$report_path" && -n "$artifact_arg" \
    && -d "$artifact_arg" \
    && -f "$artifact_arg/Dockerfile" \
    && -f "$artifact_arg/oci-metadata.json" ]]; then
    # OCI contexts have a reusable content inventory report.  Let package.py
    # verify and write it atomically so this wrapper cannot silently downgrade
    # verification to the archive-only report format below.
    "$python_bin" -I "$workspace_dir/scripts/package.py" verify \
        "${verify_args[@]}" --report "$report_path"
    exit 0
fi
"$python_bin" -I "$workspace_dir/scripts/package.py" verify "${verify_args[@]}"
if [[ -z "$report_path" ]]; then
    exit 0
fi
artifact_arg="${artifact_arg:?--report requires an archive artifact path}"
report_args=(report "$artifact_arg" --check "$report_path")
if [[ -n "$release_inventory" ]]; then
    report_args+=(--release-inventory "$release_inventory")
fi
"$python_bin" -I "$workspace_dir/scripts/package.py" "${report_args[@]}"
echo "verified package report $report_path: $(basename "$artifact_arg")"

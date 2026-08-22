#!/usr/bin/env bash
set -Eeuo pipefail

# This is a disposable, non-release supply-chain audit.  It deliberately does
# not read or write compatibility manifests and never promotes its output to a
# release artifact.

mode=audit
output=
local_key=
subjects=()

usage() {
  cat <<'EOF'
usage: supply-chain-evidence.sh [options] --subject PATH [--subject PATH ...]

Modes:
  audit     Inspect local tooling and exact subject coverage (default).
  keyless   Check prerequisites for an identity-backed Sigstore attempt;
            fail closed unless an OIDC identity and all generators exist.
  local     Sign the disposable checksum set with an explicitly supplied
            local cosign key.  This is never release evidence.

Options:
  --mode MODE       audit, keyless, or local
  --subject PATH    Subject file; may be repeated
  --local-key PATH  Local cosign key, required by --mode local
  --output PATH     Write the JSON audit beside stdout
  -h, --help        Show this help
EOF
}

while (($#)); do
  case "$1" in
    --mode)
      (($# >= 2)) || { echo "--mode requires a value" >&2; exit 64; }
      mode=$2
      shift 2
      ;;
    --subject)
      (($# >= 2)) || { echo "--subject requires a value" >&2; exit 64; }
      subjects+=("$2")
      shift 2
      ;;
    --local-key)
      (($# >= 2)) || { echo "--local-key requires a value" >&2; exit 64; }
      local_key=$2
      shift 2
      ;;
    --output)
      (($# >= 2)) || { echo "--output requires a value" >&2; exit 64; }
      output=$2
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 64
      ;;
  esac
done

case "$mode" in
  audit|keyless|local) ;;
  *) echo "invalid --mode: $mode" >&2; exit 64 ;;
esac

if ((${#subjects[@]} == 0)); then
  echo "at least one --subject is required" >&2
  exit 64
fi

python_bin=${PYTHON_BIN:-python3}
if ! command -v "$python_bin" >/dev/null 2>&1 || ! "$python_bin" --version >/dev/null 2>&1; then
  for candidate in /opt/homebrew/bin/python3 /usr/bin/python3; do
    if [[ -x "$candidate" ]] && "$candidate" --version >/dev/null 2>&1; then
      python_bin=$candidate
      break
    fi
  done
fi
if ! command -v "$python_bin" >/dev/null 2>&1 || ! "$python_bin" --version >/dev/null 2>&1; then
  echo "python3 is required for the bounded audit" >&2
  exit 2
fi

cosign_bin=${COSIGN_BIN:-}
if [[ -n "${COSIGN_BIN:-}" ]]; then
  [[ -x "$cosign_bin" ]] || cosign_bin=
else
  cosign_bin=$(command -v cosign || true)
fi
cosign_inventory_bin=$cosign_bin
if [[ -n "${COSIGN_BIN:-}" && -z "$cosign_inventory_bin" ]]; then
  cosign_inventory_bin=/__worldstream_missing_cosign__
fi

tool_json=$(
  "$python_bin" - "$cosign_inventory_bin" <<'PY'
import json
import os
import shutil
import subprocess
import sys

requested_cosign = sys.argv[1]

def tool(name, requested=None):
    path = requested if requested else shutil.which(name)
    if not path or not os.path.isfile(path) or not os.access(path, os.X_OK):
        return {"installed": False}
    version = "unavailable"
    try:
        command = [path, "version"] if name == "cosign" else [path, "--version"]
        result = subprocess.run(command, capture_output=True, text=True, timeout=10)
        text = (result.stdout + result.stderr).strip().splitlines()
        if text:
            preferred = next(
                (line.strip() for line in text if "GitVersion:" in line),
                text[0].strip(),
            )
            version = preferred[:240]
    except (OSError, subprocess.SubprocessError) as exc:
        version = f"unavailable:{type(exc).__name__}"
    return {"installed": True, "path": path, "version": version}

print(json.dumps({
    "cosign": tool("cosign", requested_cosign or None),
    "sbom_generators": {
        name: tool(name)
        for name in ("syft", "trivy", "grype", "bomctl")
    },
    "provenance_tools": {
        name: tool(name)
        for name in ("slsa-verifier", "slsa-provenance")
    },
}, sort_keys=True, separators=(",", ":")))
PY
)

work_dir=$(mktemp -d "${TMPDIR:-/tmp}/worldstream-supply-chain.XXXXXX")
trap 'rm -rf "$work_dir"' EXIT
checksums="$work_dir/checksums.sha256"
subjects_json="$work_dir/subjects.json"

"$python_bin" - "$checksums" "$subjects_json" "${subjects[@]}" <<'PY'
import hashlib
import json
import os
import sys

checksums_path, subjects_path, *raw_paths = sys.argv[1:]
root = os.getcwd()
seen = set()
rows = []
for raw in raw_paths:
    path = os.path.abspath(raw)
    if path in seen:
        raise SystemExit(f"duplicate subject: {raw}")
    seen.add(path)
    if not os.path.isfile(path):
        raise SystemExit(f"subject is not a regular file: {raw}")
    with open(path, "rb") as handle:
        data = handle.read()
    digest = hashlib.sha256(data).hexdigest()
    relative = os.path.relpath(path, root)
    rows.append({"path": relative, "absolute_path": path, "size_bytes": len(data), "sha256": digest})

with open(checksums_path, "w", encoding="utf-8", newline="\n") as handle:
    for row in rows:
        handle.write(f"{row['sha256']}  {row['path']}\n")
with open(subjects_path, "w", encoding="utf-8", newline="\n") as handle:
    json.dump(rows, handle, sort_keys=True, separators=(",", ":"))
PY

coverage_json=$(
  "$python_bin" - "$checksums" "$subjects_json" <<'PY'
import hashlib
import json
import os
import sys

checksums_path, subjects_path = sys.argv[1:]
with open(subjects_path, encoding="utf-8") as handle:
    rows = json.load(handle)
with open(checksums_path, encoding="utf-8") as handle:
    lines = [line.rstrip("\n") for line in handle if line.strip()]

observed = []
for line in lines:
    digest, separator, path = line.partition("  ")
    if separator != "  " or len(digest) != 64 or any(c not in "0123456789abcdef" for c in digest):
        raise SystemExit("invalid checksum row")
    observed.append((path, digest))
expected = [(row["path"], row["sha256"]) for row in rows]
exact = len(observed) == len(expected) and set(observed) == set(expected) and len({p for p, _ in observed}) == len(observed)
if not exact:
    raise SystemExit("subject SHA256 coverage is not exact")
print(json.dumps({
    "algorithm": "sha256",
    "exact": True,
    "subject_count": len(expected),
    "subjects": [{"path": path, "sha256": digest} for path, digest in expected],
}, sort_keys=True, separators=(",", ":")))
PY
)

oidc_source=none
if [[ -n "${SIGSTORE_ID_TOKEN:-}" || -n "${COSIGN_IDENTITY_TOKEN:-}" ]]; then
  oidc_source=explicit_token
elif [[ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" && -n "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}" ]]; then
  oidc_source=github_actions_ambient
fi

status=structural_only
reason=missing_identity_or_generators
exit_code=0
local_bundle_sha256=
local_signature_sha256=

if [[ "$mode" == keyless ]]; then
  status=blocked
  exit_code=2
  if [[ -z "$cosign_bin" ]]; then
    reason=cosign_missing
  elif [[ "$oidc_source" == none ]]; then
    reason=oidc_identity_missing
  elif ! "$python_bin" - "$tool_json" <<'PY' >/dev/null
import json, sys
tools = json.loads(sys.argv[1])
sbom = tools["sbom_generators"]
prov = tools["provenance_tools"]
raise SystemExit(0 if any(v["installed"] for v in sbom.values()) and any(v["installed"] for v in prov.values()) else 1)
PY
  then
    reason=sbom_or_provenance_generator_missing
  else
    reason=keyless_attempt_not_enabled_for_disposable_lane
  fi
elif [[ "$mode" == local ]]; then
  status=local_key_only
  reason=local_key_signature_not_release_evidence
  if [[ -z "$cosign_bin" ]]; then
    status=blocked
    reason=cosign_missing
    exit_code=2
  elif [[ -z "$local_key" || ! -f "$local_key" ]]; then
    status=blocked
    reason=local_key_missing
    exit_code=2
  else
    bundle="$work_dir/cosign.bundle.json"
    signature="$work_dir/cosign.signature"
    if ! COSIGN_PASSWORD="${COSIGN_PASSWORD:-}" "$cosign_bin" sign-blob --yes --key "$local_key" --bundle "$bundle" "$checksums" >"$signature" 2>"$work_dir/cosign.stderr"; then
      status=blocked
      reason=local_cosign_failed
      exit_code=2
    else
      local_bundle_sha256=$("$python_bin" - "$bundle" <<'PY'
import hashlib, sys
with open(sys.argv[1], "rb") as handle:
    print(hashlib.sha256(handle.read()).hexdigest())
PY
)
      local_signature_sha256=$("$python_bin" - "$signature" <<'PY'
import hashlib, sys
with open(sys.argv[1], "rb") as handle:
    print(hashlib.sha256(handle.read()).hexdigest())
PY
)
    fi
  fi
fi

report=$(
  "$python_bin" - "$tool_json" "$coverage_json" "$mode" "$status" "$reason" "$exit_code" "$oidc_source" "$local_bundle_sha256" "$local_signature_sha256" <<'PY'
import json
import sys

tools = json.loads(sys.argv[1])
coverage = json.loads(sys.argv[2])
mode, status, reason, exit_code, oidc_source, bundle_sha, signature_sha = sys.argv[3:]
print(json.dumps({
    "schema": "worldstream/non-release-supply-chain-audit/v1",
    "status": status,
    "mode": mode,
    "reason": reason,
    "exit_code": int(exit_code),
    "release_evidence": False,
    "identity_backed": False,
    "oidc_identity_source": oidc_source,
    "tools": tools,
    "subject_coverage": coverage,
    "local_cosign_bundle_sha256": bundle_sha or None,
    "local_cosign_signature_sha256": signature_sha or None,
    "manifests_read": [],
    "manifests_written": [],
}, sort_keys=True, indent=2))
PY
)

if [[ -n "$output" ]]; then
  mkdir -p "$(dirname "$output")"
  printf '%s\n' "$report" >"$output"
fi
printf '%s\n' "$report"
exit "$exit_code"

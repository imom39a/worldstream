#!/usr/bin/env bash
set -Eeuo pipefail

# This lane is diagnostic evidence only.  It never edits a compatibility
# manifest and never upgrades a cross-built artifact to native evidence.
readonly SCHEMA="worldstream/cross-platform-evidence/v1"
readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_CONFIGURATION=12
readonly EXIT_INCOMPLETE=13
readonly EXIT_RUNTIME=14
readonly ALPINE_AMD64_REF="alpine:3.22.1@sha256:4bcff63911fcb4448bd4fdacec207030997caf25e9bea4045fa6c8c44de311d1"

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
report_path="/tmp/luna-cross-platform-report.txt"
keep_image=0
run_docker=1
started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

usage() {
  cat <<'USAGE'
Usage: scripts/cross-platform-evidence.sh [options]

Probe Docker's linux/amd64 emulation with a pinned, disposable Alpine image,
exercise the existing package/OCI boundary scripts, and write the exact
commands/results to /tmp/luna-cross-platform-report.txt.

Options:
  --report PATH       Write the report to PATH instead of /tmp/luna-cross-platform-report.txt
  --no-docker         Run static/package boundary checks only
  --keep-image        Keep the disposable probe image
  --help              Show this help
USAGE
}

while (($#)); do
  case "$1" in
    --report) report_path="${2:?--report requires PATH}"; shift 2 ;;
    --no-docker) run_docker=0; shift ;;
    --keep-image) keep_image=1; shift ;;
    --help|-h) usage; exit "$EXIT_PASS" ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit "$EXIT_CONFIGURATION" ;;
  esac
done

mkdir -p "$(dirname "$report_path")"
: > "$report_path"
exec > >(tee -a "$report_path") 2>&1

echo "schema=$SCHEMA"
echo "started_at=$started_at"
echo "workspace=$workspace_dir"
echo "host.uname=$(uname -a)"
echo "host.system=$(uname -s)"
echo "host.machine=$(uname -m)"

status=0
docker_status="INCOMPLETE"
windows_status="INCOMPLETE"
linux_native_status="INCOMPLETE"
linux_reason="native_linux_x86_64_host_required"
windows_reason="native_windows_host_required"
probe_image="cross-platform-evidence-$$"
probe_tag="${probe_image}:amd64"
probe_dir=""
base_ref=""
base_digest=""
cleanup() {
  if [[ -n "$probe_image" && "$keep_image" == 0 ]] && command -v docker >/dev/null 2>&1; then
    docker image rm "$probe_tag" >/dev/null 2>&1 || true
  fi
  if [[ -n "$probe_dir" ]]; then
    rm -rf "$probe_dir"
  fi
}
trap cleanup EXIT

run_and_record() {
  local label="$1"; shift
  echo "command[$label]=$*"
  set +e
  "$@"
  local code=$?
  set -e
  echo "exit[$label]=$code"
  return "$code"
}

echo "-- manifest boundary --"
run_and_record verify_manifest python3 "$workspace_dir/scripts/verify-manifest.py" || status=1
run_and_record package_linux_dry_run bash "$workspace_dir/scripts/package-release.sh" --target linux-x86_64 --dry-run || status=1
run_and_record package_windows_dry_run bash "$workspace_dir/scripts/package-release.sh" --target windows-x64 --dry-run || status=1
run_and_record package_oci_dry_run bash "$workspace_dir/scripts/package-oci.sh" --dry-run || status=1
run_and_record package_static_smoke python3 "$workspace_dir/tests/package_smoke.py" || status=1
run_and_record oci_static_smoke python3 "$workspace_dir/tests/oci_runtime_smoke.py" || status=1
echo "manifest_claims=native_linux_x86_64_and_native_windows_x64_release_profiles;_oci_linux_amd64_is_separate"
echo "release_inventory_boundary=embedded_contract_only;finished_digests=release-manifest.json"
echo "signed_artifact_rows=status_detached,digest_empty,digest_location_release-manifest.json"
echo "sigstore_artifact_row=status_verification_material,path_only_release-manifest.json"
echo "native_linux_status=$linux_native_status"
echo "native_linux_reason=$linux_reason"
echo "windows_status=$windows_status"
echo "windows_reason=$windows_reason"
echo "cross_compilation_is_not_native_evidence=true"

if ((run_docker == 0)); then
  echo "docker_status=INCOMPLETE"
  echo "docker_reason=explicitly_disabled"
  echo "linux_amd64_emulation_status=INCOMPLETE"
  echo "linux_amd64_emulation_reason=docker_probe_disabled"
  echo "windows_native_status=INCOMPLETE"
  echo "windows_native_reason=$windows_reason"
  echo "release_evidence=false"
  echo "status=INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi

if ! command -v docker >/dev/null 2>&1; then
  echo "docker_status=UNAVAILABLE"
  echo "docker_reason=docker_command_missing"
  echo "linux_amd64_emulation_status=UNAVAILABLE"
  echo "linux_amd64_emulation_reason=docker_command_missing"
  echo "windows_native_status=INCOMPLETE"
  echo "windows_native_reason=$windows_reason"
  echo "release_evidence=false"
  echo "status=UNAVAILABLE"
  exit "$EXIT_UNAVAILABLE"
fi

if ! docker info >/dev/null 2>&1; then
  echo "docker_status=UNAVAILABLE"
  echo "docker_reason=docker_daemon_unavailable"
  echo "linux_amd64_emulation_status=UNAVAILABLE"
  echo "linux_amd64_emulation_reason=docker_daemon_unavailable"
  echo "windows_native_status=INCOMPLETE"
  echo "windows_native_reason=$windows_reason"
  echo "release_evidence=false"
  echo "status=UNAVAILABLE"
  exit "$EXIT_UNAVAILABLE"
fi

echo "-- Docker tool identity --"
run_and_record docker_version docker version || status=1
run_and_record buildx_version docker buildx version || status=1
run_and_record buildx_inspect docker buildx inspect --bootstrap || status=1
docker_server_arch="$(docker info --format '{{.Architecture}}' 2>/dev/null || true)"
docker_server_os="$(docker info --format '{{.OSType}}' 2>/dev/null || true)"
echo "docker.server_arch=$docker_server_arch"
echo "docker.server_os=$docker_server_os"

buildx_platform_output="$(docker buildx inspect --bootstrap 2>&1)"
if ! grep -Eq 'linux/amd64([ ,]|$)' <<< "$buildx_platform_output"; then
  echo "docker_status=INCOMPLETE"
  echo "docker_reason=linux_amd64_builder_platform_missing"
  echo "linux_amd64_emulation_status=INCOMPLETE"
  echo "linux_amd64_emulation_reason=linux_amd64_builder_platform_missing"
  echo "windows_native_status=INCOMPLETE"
  echo "windows_native_reason=$windows_reason"
  echo "release_evidence=false"
  echo "status=INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi

if grep -Eq 'windows/amd64([ ,]|$)' <<< "$buildx_platform_output"; then
  echo "windows_container_platform=advertised"
else
  echo "windows_container_platform=not_advertised"
fi

probe_dir="$(mktemp -d /tmp/worldstream-cross-platform.XXXXXX)"
echo "probe_dir=$probe_dir"
echo "-- pinned amd64 probe image --"
run_and_record pull_alpine docker pull --platform linux/amd64 "$ALPINE_AMD64_REF" || {
  echo "docker_status=UNAVAILABLE"
  echo "docker_reason=amd64_base_image_pull_failed"
  echo "linux_amd64_emulation_status=UNAVAILABLE"
  echo "linux_amd64_emulation_reason=amd64_base_image_pull_failed"
  echo "windows_native_status=INCOMPLETE"
  echo "windows_native_reason=$windows_reason"
  echo "release_evidence=false"
  echo "status=UNAVAILABLE"
  exit "$EXIT_UNAVAILABLE"
}
base_ref="$(docker image inspect "$ALPINE_AMD64_REF" --format '{{index .RepoDigests 0}}')"
base_digest="${base_ref##*@}"
expected_base_digest="${ALPINE_AMD64_REF##*@}"
if [[ "$base_digest" != "$expected_base_digest" ]]; then
  echo "docker_status=FAIL"
  echo "docker_reason=amd64_base_image_digest_mismatch"
  echo "release_evidence=false"
  echo "status=FAIL"
  exit "$EXIT_RUNTIME"
fi
echo "base_image_ref=$base_ref"
echo "base_image_digest=$base_digest"
docker image inspect "$ALPINE_AMD64_REF" --format 'base_image_os={{.Os}} base_image_arch={{.Architecture}} base_image_id={{.Id}}'

cat > "$probe_dir/Dockerfile" <<EOF
FROM $base_ref
RUN printf '%s\\n' worldstream-linux-amd64-emulation > /platform-proof
CMD ["/bin/sh", "-c", "uname -s -m; cat /platform-proof"]
EOF
echo "probe_dockerfile=$(tr '\n' ';' < "$probe_dir/Dockerfile")"
run_and_record build_amd64 docker buildx build --platform linux/amd64 --load --tag "$probe_tag" "$probe_dir" || {
  echo "docker_status=FAIL"
  echo "docker_reason=linux_amd64_build_failed"
  echo "linux_amd64_emulation_status=FAIL"
  echo "linux_amd64_emulation_reason=linux_amd64_build_failed"
  echo "windows_native_status=INCOMPLETE"
  echo "windows_native_reason=$windows_reason"
  echo "release_evidence=false"
  echo "status=FAIL"
  exit "$EXIT_RUNTIME"
}

echo "-- amd64 image identity and runtime --"
docker image inspect "$probe_tag" --format 'probe_image_id={{.Id}} probe_image_os={{.Os}} probe_image_arch={{.Architecture}} probe_image_digest={{if .RepoDigests}}{{index .RepoDigests 0}}{{else}}unpublished-local-image{{end}}'
run_and_record run_amd64 docker run --rm --platform linux/amd64 "$probe_tag" || {
  echo "docker_status=FAIL"
  echo "docker_reason=linux_amd64_runtime_failed"
  echo "linux_amd64_emulation_status=FAIL"
  echo "linux_amd64_emulation_reason=linux_amd64_runtime_failed"
  echo "windows_native_status=INCOMPLETE"
  echo "windows_native_reason=$windows_reason"
  echo "release_evidence=false"
  echo "status=FAIL"
  exit "$EXIT_RUNTIME"
}

probe_arch="$(docker image inspect "$probe_tag" --format '{{.Architecture}}')"
if [[ "$docker_server_arch" == "aarch64" && "$probe_arch" == "amd64" ]]; then
  docker_status="PASS"
  echo "docker_status=$docker_status"
  echo "docker_reason=linux_amd64_image_built_and_ran_on_arm64_docker_host"
  echo "linux_amd64_emulation_status=PASS"
  echo "linux_amd64_emulation_reason=arm64_host_to_amd64_container_runtime_verified"
else
  docker_status="INCOMPLETE"
  echo "docker_status=$docker_status"
  echo "docker_reason=host_or_image_architecture_did_not_prove_emulation"
  echo "linux_amd64_emulation_status=INCOMPLETE"
  echo "linux_amd64_emulation_reason=host_or_image_architecture_did_not_prove_emulation"
  status=1
fi

echo "-- existing OCI runtime boundary --"
run_and_record oci_runtime_template bash "$workspace_dir/scripts/oci-runtime-smoke.sh" --context "$workspace_dir/packaging/oci" || true
echo "existing_oci_runtime_claim=not_made_when_template_digest_or_release_metadata_is_unresolved"

echo "windows_native_status=INCOMPLETE"
echo "windows_native_reason=$windows_reason"
echo "windows_container_runtime_status=INCOMPLETE"
echo "windows_container_runtime_reason=linux_docker_host_does_not_provide_native_windows_runtime"
echo "native_linux_status=INCOMPLETE"
echo "native_linux_reason=$linux_reason"
echo "release_evidence=false"
if ((status == 0)); then
  echo "status=INCOMPLETE"
  echo "overall_reason=linux_amd64_oci_emulation_verified_but_native_release_cells_remain_unproven"
  exit "$EXIT_INCOMPLETE"
fi
echo "status=FAIL"
exit "$EXIT_RUNTIME"

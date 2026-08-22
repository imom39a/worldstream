#!/usr/bin/env bash
set -euo pipefail

readonly SCHEMA="worldstream/oci-runtime-smoke/v1"
readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_CONFIGURATION=12
readonly EXIT_INCOMPLETE=13
readonly EXIT_RUNTIME=14
readonly POSTGRES_IMAGE="postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly POSTGRES_IDENTITY="postgresql/17.11; server_version_num=170011"
readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

context_dir=
image_tag="worldstream-oci-smoke:$$"
keep_image=0
skip_build=0
stage="configuration"
report_path=""
artifact_path=""
secret_scan_root=""
secret_sentinel_file=""

emit_report() {
  local payload="$1"
  printf '%s\n' "$payload"
  if [[ -n "$report_path" ]]; then
    if [[ -L "$report_path" || -d "$report_path" ]]; then
      printf 'unsafe OCI runtime report path: %s\n' "$report_path" >&2
      return 1
    fi
    mkdir -p "$(dirname "$report_path")"
    local temporary
    temporary="$(mktemp "$(dirname "$report_path")/.${report_path##*/}.XXXXXX")"
    printf '%s\n' "$payload" >"$temporary"
    chmod 0644 "$temporary"
    mv -f "$temporary" "$report_path"
  fi
}

report() {
  local status="$1"
  local reason="$2"
  local payload
  payload="$(printf '{"schema":"%s","status":"%s","reason":"%s","release_evidence":false}' \
    "$SCHEMA" "$status" "$reason")"
  emit_report "$payload"
}

on_error() {
  report "FAIL" "$stage"
  exit "$EXIT_RUNTIME"
}

trap on_error ERR

while (($#)); do
  case "$1" in
    --context) context_dir="${2:?--context requires DIR}"; shift 2 ;;
    --image) image_tag="${2:?--image requires TAG}"; shift 2 ;;
    --artifact) artifact_path="${2:?--artifact requires FILE}"; shift 2 ;;
    --report) report_path="${2:?--report requires PATH}"; shift 2 ;;
    --keep-image) keep_image=1; shift ;;
    --skip-build) skip_build=1; shift ;;
    --help|-h)
      echo 'Usage: scripts/oci-runtime-smoke.sh --context DIR [--artifact OCI_TAR] [--image TAG] [--report PATH] [--keep-image] [--skip-build]'
      exit "$EXIT_PASS"
      ;;
    *) echo "unknown argument: $1" >&2; exit "$EXIT_CONFIGURATION" ;;
  esac
done
if [[ -z "$context_dir" ]]; then
  echo '--context is required' >&2
  exit "$EXIT_CONFIGURATION"
fi
if [[ ! -d "$context_dir" || -L "$context_dir" ]]; then
  report "FAIL" "context_not_a_directory"
  exit "$EXIT_CONFIGURATION"
fi
context_dir="$(cd "$context_dir" && pwd)"
if [[ -n "$artifact_path" ]]; then
  if [[ ! -f "$artifact_path" || -L "$artifact_path" ]]; then
    report "FAIL" "artifact_not_a_regular_file"
    exit "$EXIT_CONFIGURATION"
  fi
  artifact_path="$(cd "$(dirname "$artifact_path")" && pwd)/${artifact_path##*/}"
fi
if ((skip_build == 1)) && [[ -z "$artifact_path" ]]; then
  report "FAIL" "skip_build_requires_artifact"
  exit "$EXIT_CONFIGURATION"
fi
for required_file in Dockerfile entrypoint.sh oci-metadata.json; do
  if [[ ! -f "$context_dir/$required_file" || -L "$context_dir/$required_file" ]]; then
    report "FAIL" "context_missing_required_file"
    exit "$EXIT_CONFIGURATION"
  fi
done

if ! command -v docker >/dev/null 2>&1; then
  report "UNAVAILABLE" "docker_unavailable"
  exit "$EXIT_UNAVAILABLE"
fi
if ! docker info >/dev/null 2>&1; then
  report "UNAVAILABLE" "docker_daemon_unavailable"
  exit "$EXIT_UNAVAILABLE"
fi
if ! docker buildx inspect >/dev/null 2>&1; then
  report "UNAVAILABLE" "docker_buildx_unavailable"
  exit "$EXIT_UNAVAILABLE"
fi

python_bin="$(command -v python3 2>/dev/null || true)"
if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then
  report "UNAVAILABLE" "python3_unavailable"
  exit "$EXIT_UNAVAILABLE"
fi
metadata_output=""
metadata_reason=""
if ! metadata_output="$("$python_bin" - "$context_dir/oci-metadata.json" 2>/dev/null <<'PY'
import json
import re
import sys


def reject(reason: str) -> None:
    print(f"FAIL:{reason}")
    raise SystemExit(1)


try:
    with open(sys.argv[1], encoding="utf-8") as source:
        metadata = json.load(source)
except (OSError, UnicodeDecodeError, json.JSONDecodeError):
    reject("context_metadata_unreadable")

if not isinstance(metadata, dict):
    reject("context_metadata_not_an_object")

base = metadata.get("base_image")
if not isinstance(base, str) or not re.fullmatch(r"[^\s@]+@sha256:[0-9a-f]{64}", base):
    reject("pinned_base_image_required")
version = metadata.get("version")
manifest_sha256 = metadata.get("manifest_sha256")
source_revision = metadata.get("source_revision")
build_identity_sha256 = metadata.get("build_identity_sha256")
if not isinstance(version, str) or not version or any(ord(char) < 32 for char in version):
    reject("version_metadata_invalid")
if not isinstance(manifest_sha256, str) or not re.fullmatch(r"[0-9a-f]{64}", manifest_sha256):
    reject("manifest_digest_required")
if not isinstance(source_revision, str) or not re.fullmatch(r"[0-9a-f]{40}", source_revision):
    reject("source_revision_required")
if not isinstance(build_identity_sha256, str) or not re.fullmatch(
    r"sha256:[0-9a-f]{64}", build_identity_sha256
):
    reject("build_identity_digest_required")
if metadata.get("artifact") != "worldstream-oci/v1":
    reject("metadata_artifact_identity_invalid")
if metadata.get("profile") != "oci-linux-amd64" or metadata.get("target") != "linux/amd64":
    reject("metadata_target_identity_invalid")

runtime = metadata.get("runtime")
if not isinstance(runtime, dict):
    reject("runtime_policy_missing")
if (
    runtime.get("uid") != 65532
    or runtime.get("gid") != 65532
    or runtime.get("read_only_root") is not True
    or runtime.get("volume") != "/var/lib/worldstream"
    or runtime.get("healthcheck") != "worldstreamctl --data-dir /var/lib/worldstream health"
    or runtime.get("sqlite_filesystems") != ["ext4", "xfs"]
):
    reject("runtime_policy_invalid")
deny = runtime.get("reject_filesystems")
if (
    not isinstance(deny, list)
    or any(not isinstance(item, str) for item in deny)
    or not {"overlay", "tmpfs", "nfs", "cifs", "fuse", "fuseblk", "smb"}.issubset(set(deny))
):
    reject("runtime_filesystem_deny_list_invalid")

permissions = metadata.get("permissions")
if not isinstance(permissions, dict) or permissions.get("user") != "65532:65532":
    reject("runtime_identity_invalid")

print(base)
print(version)
print(manifest_sha256)
print(source_revision)
print(build_identity_sha256)
PY
)"; then
  if [[ -z "$metadata_output" ]]; then
    report "UNAVAILABLE" "python3_execution_failed"
    exit "$EXIT_UNAVAILABLE"
  fi
  metadata_reason="${metadata_output#FAIL:}"
  case "$metadata_reason" in
    context_metadata_unreadable|context_metadata_not_an_object|pinned_base_image_required|version_metadata_invalid|manifest_digest_required|source_revision_required|build_identity_digest_required|metadata_artifact_identity_invalid|metadata_target_identity_invalid|runtime_policy_missing|runtime_policy_invalid|runtime_filesystem_deny_list_invalid|runtime_identity_invalid) ;;
    *) metadata_reason="context_metadata_invalid" ;;
  esac
  report "INCOMPLETE" "$metadata_reason"
  exit "$EXIT_INCOMPLETE"
fi
mapfile -t metadata_values <<< "$metadata_output"
if [[ "${#metadata_values[@]}" -ne 5 ]]; then
  report "FAIL" "invalid_context_metadata_shape"
  exit "$EXIT_CONFIGURATION"
fi
base_image="${metadata_values[0]}"
version="${metadata_values[1]}"
manifest_sha256="${metadata_values[2]}"
source_revision="${metadata_values[3]}"
build_identity_sha256="${metadata_values[4]}"

validate_profile_probes() {
  local health_json="$1"
  local ready_json="$2"
  local version_json="$3"
  local profile="$4"
  local expected_engine_identity="${5:-}"
  "$python_bin" - \
    "$SCRIPT_DIR/package.py" \
    "$context_dir/manifest/compatibility.json" \
    "$health_json" \
    "$ready_json" \
    "$version_json" \
    "$profile" \
    "$expected_engine_identity" \
    "$source_revision" <<'PY'
import importlib.util
import json
import pathlib
import sys

package_path = pathlib.Path(sys.argv[1])
spec = importlib.util.spec_from_file_location("worldstream_oci_probe_package", package_path)
if spec is None or spec.loader is None:
    raise SystemExit(1)
package = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = package
spec.loader.exec_module(package)

with pathlib.Path(sys.argv[2]).open("rb") as source:
    manifest = json.load(source)
health = json.loads(sys.argv[3])
ready = json.loads(sys.argv[4])
version = json.loads(sys.argv[5])
profile = sys.argv[6]
expected_engine_identity = sys.argv[7]
expected_source_revision = sys.argv[8]

if health != {"status": "ok"} or ready != {"status": "ready"}:
    raise SystemExit(1)
required_fields = {
    "product_build",
    "wire",
    "config",
    "storage_schema",
    "core_schema_version",
    "hash_suite",
    "manifest",
    "engine",
}
if not isinstance(version, dict) or set(version) != required_fields:
    raise SystemExit(1)
contracts = manifest.get("contracts")
product_build = version.get("product_build")
if not isinstance(contracts, dict) or not isinstance(product_build, dict):
    raise SystemExit(1)
product = contracts.get("product")
if product_build != {
    "product": product,
    "binary": "worldstreamd",
    "build_version": product,
    "source_revision": expected_source_revision,
}:
    raise SystemExit(1)
for field in ("wire", "config", "storage_schema", "core_schema_version", "hash_suite"):
    if version.get(field) != contracts.get(field):
        raise SystemExit(1)
if version.get("manifest") != package.canonical_runtime_manifest_summary(manifest):
    raise SystemExit(1)
engine = version.get("engine")
if not isinstance(engine, dict) or set(engine) != {
    "profile",
    "status",
    "exact_identity",
}:
    raise SystemExit(1)
package.validate_runtime_engine_identity(engine, manifest, profile)
if expected_engine_identity and engine.get("exact_identity") != expected_engine_identity:
    raise SystemExit(1)
rendered = json.dumps((health, ready, version), sort_keys=True).lower()
if "fallback" in rendered or "degraded" in rendered:
    raise SystemExit(1)
print(engine["exact_identity"])
PY
}

volume_name="worldstream-oci-smoke-volume-$$"
secret_volume_name="worldstream-oci-smoke-secret-$$"
postgres_socket_volume_name="worldstream-oci-smoke-postgres-socket-$$"
container_name="worldstream-oci-smoke-container-$$"
postgres_container_name="worldstream-oci-smoke-postgres-$$"
container_diagnostic() {
  docker inspect -f 'status={{.State.Status}} exit={{.State.ExitCode}} oom={{.State.OOMKilled}} error={{json .State.Error}}' "$container_name" >&2 2>/dev/null || true
  docker logs --tail 40 "$container_name" >&2 2>/dev/null || true
}
cleanup() {
  docker rm -f "$container_name" >/dev/null 2>&1 || true
  docker rm -f "$postgres_container_name" >/dev/null 2>&1 || true
  docker volume rm "$volume_name" >/dev/null 2>&1 || true
  docker volume rm "$secret_volume_name" >/dev/null 2>&1 || true
  docker volume rm "$postgres_socket_volume_name" >/dev/null 2>&1 || true
  if ((keep_image == 0)); then docker image rm "$image_tag" >/dev/null 2>&1 || true; fi
  if [[ -d "$secret_scan_root" \
    && "${secret_scan_root##*/}" == worldstream-oci-secret-scan.* ]]; then
    rm -rf -- "$secret_scan_root"
  fi
}
trap cleanup EXIT

secret_scan_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-oci-secret-scan.XXXXXX")"
chmod 0700 "$secret_scan_root"
secret_sentinel_file="$secret_scan_root/sentinel"
"$python_bin" - "$secret_sentinel_file" <<'PY'
import os
import secrets
import sys

path = sys.argv[1]
descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(descriptor, "wb") as output:
    output.write(secrets.token_hex(16).encode("ascii"))
    output.flush()
    os.fsync(output.fileno())
PY

stage="image_build"
if ((skip_build == 1)); then
  docker image inspect "$image_tag" >/dev/null
else
  docker buildx build --load --platform linux/amd64 \
    --build-arg "WORLDSTREAM_BASE_IMAGE=$base_image" \
    --build-arg "VERSION=$version" \
    --build-arg "MANIFEST_SHA256=$manifest_sha256" \
    --build-arg "SOURCE_REVISION=$source_revision" \
    --build-arg "BUILD_IDENTITY_SHA256=$build_identity_sha256" \
    --build-arg "SOURCE_DATE_EPOCH=0" \
    --tag "$image_tag" "$context_dir"
fi

stage="image_configuration"
"$python_bin" - "$image_tag" 2>/dev/null <<'PY'
import json
import subprocess
import sys

image = sys.argv[1]
record = json.loads(subprocess.check_output(["docker", "image", "inspect", image]))[0]
config = record["Config"]
if config.get("User") != "65532:65532":
    raise SystemExit(1)
if config.get("Volumes") != {"/var/lib/worldstream": {}}:
    raise SystemExit(1)
health = config.get("Healthcheck", {}).get("Test", [])
if not any("worldstreamctl" in part for part in health):
    raise SystemExit(1)
labels = config.get("Labels") or {}
for key, value in {
    "io.worldstream.target": "linux/amd64",
    "io.worldstream.read-only-root": "true",
    "io.worldstream.sqlite-filesystems": "ext4,xfs",
}.items():
    if labels.get(key) != value:
        raise SystemExit(1)
if record.get("Os") != "linux" or record.get("Architecture") != "amd64":
    raise SystemExit(1)
PY
docker image inspect "$image_tag" >"$secret_scan_root/image-config.json"
docker history --no-trunc --format '{{json .}}' "$image_tag" \
  >"$secret_scan_root/image-history.jsonl"

artifact_binding='{"status":"not_requested"}'
if [[ -n "$artifact_path" ]]; then
  stage="artifact_binding"
  tested_image_id="$(docker image inspect --format '{{.Id}}' "$image_tag")"
  artifact_binding="$("$python_bin" "$SCRIPT_DIR/verify-oci-layout.py" \
    --artifact "$artifact_path" \
    --tested-image-id "$tested_image_id")"
fi

stage="negative_policy"
ephemeral_status=0
ephemeral_output="$(docker run --rm --read-only --mount type=tmpfs,destination=/var/lib/worldstream "$image_tag" 2>&1)" || ephemeral_status=$?
wrong_path_status=0
wrong_path_output="$(docker run --rm --read-only -e WORLDSTREAM__STORAGE__DATA_DIR=/tmp "$image_tag" 2>&1)" || wrong_path_status=$?
printf '%s\n%s\n' "$ephemeral_output" "$wrong_path_output" \
  >"$secret_scan_root/negative-probe-output.log"
if [[ "$ephemeral_status" != 78 || "$ephemeral_output" != *filesystem* ]]; then
  report "FAIL" "tmpfs_policy_not_rejected"
  exit "$EXIT_RUNTIME"
fi
if [[ "$wrong_path_status" != 78 || "$wrong_path_output" != *"data directory"* ]]; then
  report "FAIL" "non_canonical_data_directory_not_rejected"
  exit "$EXIT_RUNTIME"
fi

stage="volume_probe"
docker volume create "$volume_name" >/dev/null
docker run --rm --user 0:0 --entrypoint sh \
  --mount "type=volume,source=$volume_name,target=/var/lib/worldstream" \
  "$image_tag" -eu -c 'chown 65532:65532 /var/lib/worldstream; chmod 0700 /var/lib/worldstream'
volume_filesystem="$(docker run --rm --entrypoint sh \
  --mount "type=volume,source=$volume_name,target=/var/lib/worldstream" \
  "$image_tag" -eu -c 'awk '\''$5 == "/var/lib/worldstream" { for (field = 6; field <= NF; field += 1) if ($field == "-" && field < NF) print $(field + 1) }'\'' /proc/self/mountinfo | tail -n 1')"
case "$volume_filesystem" in
  ext4|xfs) ;;
  *)
    "$python_bin" - "$volume_filesystem" <<'PY'
import json
import sys

print(json.dumps({
    "schema": "worldstream/oci-runtime-smoke/v1",
    "status": "INCOMPLETE",
    "reason": "local_volume_filesystem_outside_allow_list",
    "filesystem": sys.argv[1],
    "negative_policy_checks": "PASS",
    "release_evidence": False,
}, sort_keys=True))
PY
    exit "$EXIT_INCOMPLETE"
    ;;
esac

stage="authority_secret"
docker volume create "$secret_volume_name" >/dev/null
docker run --rm --interactive --user 0:0 --entrypoint sh \
  --mount "type=volume,source=$secret_volume_name,target=/run/worldstream-secrets" \
  "$image_tag" -eu -c \
  'umask 077; head -c 32 > /run/worldstream-secrets/authority.secret; test "$(wc -c < /run/worldstream-secrets/authority.secret)" -eq 32; chown 65532:65532 /run/worldstream-secrets/authority.secret; chmod 0600 /run/worldstream-secrets/authority.secret' \
  <"$secret_sentinel_file"

stage="container_startup"
docker run -d --name "$container_name" --read-only \
  --tmpfs /tmp:rw,nosuid,nodev,size=16m \
  --mount "type=volume,source=$volume_name,target=/var/lib/worldstream" \
  --mount "type=volume,source=$secret_volume_name,target=/run/worldstream-secrets,readonly" \
  --env WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE=/run/worldstream-secrets/authority.secret \
  "$image_tag" >/dev/null
for _ in {1..30}; do
  state="$(docker inspect -f '{{.State.Status}}' "$container_name" 2>/dev/null || true)"
  if [[ "$state" == running ]]; then
    break
  fi
  if [[ "$state" == exited || "$state" == dead ]]; then
    container_diagnostic
    report "FAIL" "container_exited_before_probe"
    exit "$EXIT_RUNTIME"
  fi
  sleep 1
done
if [[ "$(docker inspect -f '{{.State.Status}}' "$container_name")" != running ]]; then
  container_diagnostic
  report "FAIL" "container_start_timeout"
  exit "$EXIT_RUNTIME"
fi
if [[ "$(docker inspect -f '{{.HostConfig.ReadonlyRootfs}}' "$container_name")" != true ]]; then
  report "FAIL" "read_only_root_not_enabled"
  exit "$EXIT_RUNTIME"
fi
if [[ "$(docker exec "$container_name" id -u)" != 65532 || "$(docker exec "$container_name" id -g)" != 65532 ]]; then
  report "FAIL" "container_identity_policy_failed"
  exit "$EXIT_RUNTIME"
fi

stage="read_only_root_probe"
if docker exec "$container_name" sh -eu -c 'touch /etc/.worldstream-read-only-root-probe' >/dev/null 2>&1; then
  report "FAIL" "read_only_root_write_succeeded"
  exit "$EXIT_RUNTIME"
fi

stage="persistent_volume"
docker exec "$container_name" sh -eu -c 'test -w /var/lib/worldstream; printf persistent > /var/lib/worldstream/.oci-runtime-smoke; test "$(cat /var/lib/worldstream/.oci-runtime-smoke)" = persistent'

stage="healthcheck"
health=
for _ in {1..30}; do
  health="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}missing{{end}}' "$container_name")"
  if [[ "$health" == healthy ]]; then
    break
  fi
  sleep 1
done
if [[ "$health" != healthy ]]; then
  container_diagnostic
  report "FAIL" "healthcheck_not_healthy"
  exit "$EXIT_RUNTIME"
fi

stage="sqlite_profile_probes"
sqlite_health_json=""
sqlite_ready_json=""
sqlite_version_json=""
for _ in {1..30}; do
  sqlite_health_json="$(docker exec "$container_name" busybox wget -qO- http://127.0.0.1:9410/healthz 2>/dev/null || true)"
  sqlite_ready_json="$(docker exec "$container_name" busybox wget -qO- http://127.0.0.1:9410/readyz 2>/dev/null || true)"
  sqlite_version_json="$(docker exec "$container_name" busybox wget -qO- http://127.0.0.1:9410/version 2>/dev/null || true)"
  if [[ -n "$sqlite_health_json" && -n "$sqlite_ready_json" && -n "$sqlite_version_json" ]]; then
    break
  fi
  sleep 1
done
sqlite_engine_identity="$(validate_profile_probes \
  "$sqlite_health_json" \
  "$sqlite_ready_json" \
  "$sqlite_version_json" \
  "sqlite-bundled")"

stage="healthcheck_requires_daemon"
docker run --rm --read-only --network none --tmpfs /tmp:rw,nosuid,nodev,size=16m \
  --mount "type=volume,source=$volume_name,target=/var/lib/worldstream" \
  --mount "type=volume,source=$secret_volume_name,target=/run/worldstream-secrets,readonly" \
  --env WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE=/run/worldstream-secrets/authority.secret \
  --entrypoint /usr/local/bin/worldstreamctl \
  "$image_tag" --data-dir /var/lib/worldstream config validate >/dev/null
if docker run --rm --read-only --network none --tmpfs /tmp:rw,nosuid,nodev,size=16m \
  --mount "type=volume,source=$volume_name,target=/var/lib/worldstream" \
  --mount "type=volume,source=$secret_volume_name,target=/run/worldstream-secrets,readonly" \
  --env WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE=/run/worldstream-secrets/authority.secret \
  --entrypoint /usr/local/bin/worldstreamctl \
  "$image_tag" --data-dir /var/lib/worldstream health >/dev/null 2>&1; then
  report "FAIL" "healthcheck_succeeded_without_daemon"
  exit "$EXIT_RUNTIME"
fi

stage="postgres_provider_startup"
docker logs "$container_name" >"$secret_scan_root/sqlite-container.log" 2>&1
docker inspect "$container_name" >"$secret_scan_root/sqlite-container-config.json"
docker rm -f "$container_name" >/dev/null
docker pull "$POSTGRES_IMAGE" >/dev/null
docker volume create "$postgres_socket_volume_name" >/dev/null
docker run --detach --name "$postgres_container_name" --network none \
  --tmpfs /var/lib/postgresql/data:rw,nosuid,nodev,size=512m \
  --mount "type=volume,source=$postgres_socket_volume_name,target=/var/run/postgresql" \
  --env POSTGRES_USER=admin \
  --env POSTGRES_DB=worldstream \
  --env POSTGRES_HOST_AUTH_METHOD=trust \
  "$POSTGRES_IMAGE" \
  -c listen_addresses= \
  -c unix_socket_directories=/var/run/postgresql >/dev/null
postgres_ready=0
for _ in {1..60}; do
  if docker exec "$postgres_container_name" pg_isready \
    --host /var/run/postgresql --username admin --dbname worldstream >/dev/null 2>&1; then
    postgres_ready=1
    break
  fi
  sleep 1
done
if ((postgres_ready != 1)); then
  docker logs --tail 40 "$postgres_container_name" >&2 2>/dev/null || true
  report "FAIL" "postgres_provider_not_ready"
  exit "$EXIT_RUNTIME"
fi
postgres_version="$(docker exec "$postgres_container_name" psql \
  --host /var/run/postgresql --username admin --dbname worldstream \
  --no-psqlrc --quiet --no-align --tuples-only --command 'SHOW server_version_num' | tr -d '[:space:]')"
if [[ "$postgres_version" != 170011 ]]; then
  report "FAIL" "postgres_provider_version_mismatch"
  exit "$EXIT_RUNTIME"
fi

stage="postgres_secret_inputs"
docker run --rm --user 0:0 --entrypoint sh \
  --mount "type=volume,source=$secret_volume_name,target=/run/worldstream-secrets" \
  "$image_tag" -eu -c '
    umask 077
    printf "%s\n" "host=/var/run/postgresql dbname=worldstream user=admin sslmode=disable" > /run/worldstream-secrets/postgres-admin.dsn
    printf "%s\n" "host=/var/run/postgresql dbname=worldstream user=runtime sslmode=disable" > /run/worldstream-secrets/postgres-runtime.dsn
    chown 65532:65532 /run/worldstream-secrets/postgres-admin.dsn /run/worldstream-secrets/postgres-runtime.dsn
    chmod 0600 /run/worldstream-secrets/postgres-admin.dsn /run/worldstream-secrets/postgres-runtime.dsn
  '

stage="postgres_runtime_role_prepare"
docker exec -i "$postgres_container_name" psql \
  --host /var/run/postgresql --username admin --dbname worldstream \
  --no-psqlrc --quiet --set ON_ERROR_STOP=1 >/dev/null <<'SQL'
CREATE ROLE runtime LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT;
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT CONNECT ON DATABASE worldstream TO runtime;
GRANT USAGE ON SCHEMA public TO runtime;
ALTER DEFAULT PRIVILEGES FOR ROLE admin IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO runtime;
ALTER DEFAULT PRIVILEGES FOR ROLE admin IN SCHEMA public GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO runtime;
SQL

stage="postgres_packaged_admin"
postgres_admin_common=(
  docker run --rm --network none --read-only
  --tmpfs /tmp:rw,nosuid,nodev,size=16m
  --mount "type=volume,source=$postgres_socket_volume_name,target=/var/run/postgresql,readonly"
  --mount "type=volume,source=$secret_volume_name,target=/run/worldstream-secrets,readonly"
  --entrypoint /usr/local/bin/worldstreamctl
  "$image_tag" postgres
)
migrate_output="$("${postgres_admin_common[@]}" migrate --dsn-file /run/worldstream-secrets/postgres-admin.dsn)"
verify_output="$("${postgres_admin_common[@]}" verify --dsn-file /run/worldstream-secrets/postgres-admin.dsn)"
"$python_bin" - "$migrate_output" "$verify_output" <<'PY'
import json
import sys

for expected, raw in zip(("migrate", "verify"), sys.argv[1:], strict=True):
    value = json.loads(raw)
    if value != {
        "status": "ok",
        "operation": expected,
        "connection": "direct-admin",
        "release_evidence": False,
    }:
        raise SystemExit(1)
PY

stage="postgres_runtime_role_hardening"
docker exec -i "$postgres_container_name" psql \
  --host /var/run/postgresql --username admin --dbname worldstream \
  --no-psqlrc --quiet --set ON_ERROR_STOP=1 >/dev/null <<'SQL'
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO runtime;
GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO runtime;
REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE public.worldstream_schema_migrations FROM runtime;
SQL
runtime_role="$(docker exec "$postgres_container_name" psql \
  --host /var/run/postgresql --username runtime --dbname worldstream \
  --no-psqlrc --quiet --no-align --tuples-only --command \
  "SELECT role.rolsuper::text || '|' || role.rolcreaterole::text || '|' || role.rolcreatedb::text || '|' || role.rolreplication::text || '|' || role.rolbypassrls::text || '|' || has_database_privilege(current_user, current_database(), 'CREATE')::text || '|' || EXISTS (SELECT 1 FROM pg_catalog.pg_auth_members AS membership WHERE membership.member = role.oid)::text || '|' || has_schema_privilege(current_user, 'public', 'CREATE')::text || '|' || EXISTS (SELECT 1 FROM pg_catalog.pg_namespace AS namespace_row WHERE namespace_row.nspname = 'public' AND namespace_row.nspowner = role.oid UNION ALL SELECT 1 FROM pg_catalog.pg_class AS relation_row JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = relation_row.relnamespace WHERE namespace_row.nspname = 'public' AND relation_row.relowner = role.oid UNION ALL SELECT 1 FROM pg_catalog.pg_proc AS routine_row JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = routine_row.pronamespace WHERE namespace_row.nspname = 'public' AND routine_row.proowner = role.oid UNION ALL SELECT 1 FROM pg_catalog.pg_type AS type_row JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = type_row.typnamespace WHERE namespace_row.nspname = 'public' AND type_row.typowner = role.oid)::text || '|' || has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'INSERT')::text || '|' || has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'UPDATE')::text || '|' || has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'DELETE')::text || '|' || has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'TRUNCATE')::text FROM pg_catalog.pg_roles AS role WHERE role.rolname = current_user" | tr -d '[:space:]')"
if [[ "$runtime_role" != "false|false|false|false|false|false|false|false|false|false|false|false|false" ]]; then
  report "FAIL" "postgres_runtime_role_not_least_privileged"
  exit "$EXIT_RUNTIME"
fi

stage="postgres_profile_startup"
docker run -d --name "$container_name" --network none --read-only \
  --tmpfs /tmp:rw,nosuid,nodev,size=16m \
  --mount "type=volume,source=$volume_name,target=/var/lib/worldstream" \
  --mount "type=volume,source=$secret_volume_name,target=/run/worldstream-secrets,readonly" \
  --mount "type=volume,source=$postgres_socket_volume_name,target=/var/run/postgresql,readonly" \
  --env WORLDSTREAM__STORAGE__PROFILE=postgres-primary \
  --env WORLDSTREAM__STORAGE__POSTGRESQL__DSN_FILE=/run/worldstream-secrets/postgres-runtime.dsn \
  --env WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE=/run/worldstream-secrets/authority.secret \
  "$image_tag" >/dev/null
for _ in {1..60}; do
  state="$(docker inspect -f '{{.State.Status}}' "$container_name" 2>/dev/null || true)"
  if [[ "$state" == running ]]; then
    break
  fi
  if [[ "$state" == exited || "$state" == dead ]]; then
    container_diagnostic
    report "FAIL" "postgres_profile_exited_before_probe"
    exit "$EXIT_RUNTIME"
  fi
  sleep 1
done
if [[ "$(docker inspect -f '{{.State.Status}}' "$container_name")" != running ]]; then
  container_diagnostic
  report "FAIL" "postgres_profile_start_timeout"
  exit "$EXIT_RUNTIME"
fi
if [[ "$(docker inspect -f '{{.HostConfig.ReadonlyRootfs}}' "$container_name")" != true ]]; then
  report "FAIL" "postgres_profile_read_only_root_not_enabled"
  exit "$EXIT_RUNTIME"
fi
if [[ "$(docker exec "$container_name" id -u)" != 65532 || "$(docker exec "$container_name" id -g)" != 65532 ]]; then
  report "FAIL" "postgres_profile_identity_policy_failed"
  exit "$EXIT_RUNTIME"
fi

stage="postgres_profile_probes"
postgres_health=0
postgres_health_json=""
postgres_ready_json=""
postgres_version_json=""
for _ in {1..60}; do
  if docker exec "$container_name" /usr/local/bin/worldstreamctl \
    --data-dir /var/lib/worldstream health >/dev/null 2>&1; then
    postgres_health=1
    postgres_health_json="$(docker exec "$container_name" busybox wget -qO- http://127.0.0.1:9410/healthz 2>/dev/null || true)"
    postgres_ready_json="$(docker exec "$container_name" busybox wget -qO- http://127.0.0.1:9410/readyz 2>/dev/null || true)"
    postgres_version_json="$(docker exec "$container_name" busybox wget -qO- http://127.0.0.1:9410/version 2>/dev/null || true)"
    if [[ -n "$postgres_health_json" && -n "$postgres_ready_json" && -n "$postgres_version_json" ]]; then
      break
    fi
  fi
  sleep 1
done
if ((postgres_health != 1)); then
  container_diagnostic
  report "FAIL" "postgres_profile_health_probe_failed"
  exit "$EXIT_RUNTIME"
fi
postgres_engine_identity="$(validate_profile_probes \
  "$postgres_health_json" \
  "$postgres_ready_json" \
  "$postgres_version_json" \
  "postgres-primary" \
  "$POSTGRES_IDENTITY")"
if [[ "$postgres_engine_identity" != "$POSTGRES_IDENTITY" ]]; then
  report "FAIL" "postgres_profile_engine_identity_mismatch"
  exit "$EXIT_RUNTIME"
fi

docker logs "$container_name" >"$secret_scan_root/postgres-container.log" 2>&1
docker inspect "$container_name" >"$secret_scan_root/postgres-container-config.json"
docker logs "$postgres_container_name" >"$secret_scan_root/provider-container.log" 2>&1
docker inspect "$postgres_container_name" >"$secret_scan_root/provider-container-config.json"
printf '%s\n' \
  "sqlite_health=$sqlite_health_json" \
  "sqlite_ready=$sqlite_ready_json" \
  "sqlite_version=$sqlite_version_json" \
  "postgres_health=$postgres_health_json" \
  "postgres_ready=$postgres_ready_json" \
  "postgres_version=$postgres_version_json" \
  "migrate=$migrate_output" \
  "verify=$verify_output" \
  >"$secret_scan_root/probe-output.log"

"$python_bin" - "$image_tag" "$health" "$artifact_binding" "$sqlite_engine_identity" "$POSTGRES_IMAGE" "$POSTGRES_IDENTITY" >"$secret_scan_root/runtime-report-candidate.json" <<'PY'
import json
import sys

print(json.dumps({
    "schema": "worldstream/oci-runtime-smoke/v1",
    "status": "PASS",
    "image": sys.argv[1],
    "health": sys.argv[2],
    "read_only_root": True,
    "non_root": "65532:65532",
    "persistent_volume": "/var/lib/worldstream",
    "authority_secret_source": "owner-readable-read-only-volume-file",
    "standalone_config": "valid",
    "rejected_layouts": ["tmpfs", "wrong-data-directory"],
    "healthcheck_without_daemon": "rejected",
    "artifact_binding": json.loads(sys.argv[3]),
    "profiles": {
        "sqlite-bundled": {
            "status": "pass",
            "health": "healthy",
            "filesystem": "ext4-or-xfs-explicit-volume",
            "engine_identity": sys.argv[4],
            "healthz": "pass",
            "readyz": "pass",
            "version": "pass",
        },
        "postgres-primary": {
            "status": "pass",
            "provider_image": sys.argv[5],
            "engine_identity": sys.argv[6],
            "packaged_admin_migrate": "pass",
            "packaged_admin_verify": "pass",
            "runtime_role_least_privilege": True,
            "healthz": "pass",
            "readyz": "pass",
            "version": "pass",
            "network": "disabled-unix-socket",
        },
    },
    "secrets_emitted": "pending_scan",
    "release_evidence": False,
}, sort_keys=True))
PY
"$python_bin" "$SCRIPT_DIR/verify-secret-absence.py" \
  --sentinel-file "$secret_sentinel_file" \
  --channel "image-config=$secret_scan_root/image-config.json" \
  --channel "image-history=$secret_scan_root/image-history.jsonl" \
  --channel "negative-probes=$secret_scan_root/negative-probe-output.log" \
  --channel "sqlite-container-config=$secret_scan_root/sqlite-container-config.json" \
  --channel "sqlite-container-logs=$secret_scan_root/sqlite-container.log" \
  --channel "postgres-container-config=$secret_scan_root/postgres-container-config.json" \
  --channel "postgres-container-logs=$secret_scan_root/postgres-container.log" \
  --channel "provider-container-config=$secret_scan_root/provider-container-config.json" \
  --channel "provider-container-logs=$secret_scan_root/provider-container.log" \
  --channel "probe-output=$secret_scan_root/probe-output.log" \
  --channel "runtime-report=$secret_scan_root/runtime-report-candidate.json" \
  --output "$secret_scan_root/secret-scan.json" \
  >/dev/null
final_report="$("$python_bin" - "$secret_scan_root/runtime-report-candidate.json" "$secret_scan_root/secret-scan.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    report = json.load(source)
with open(sys.argv[2], encoding="utf-8") as source:
    scan = json.load(source)
if scan.get("status") != "pass" or scan.get("secrets_emitted") is not False:
    raise SystemExit(1)
report["secrets_emitted"] = False
report["secret_scan"] = scan
print(json.dumps(report, sort_keys=True))
PY
)"
emit_report "$final_report"

#!/usr/bin/env bash
set -euo pipefail

# Provider-native PostgreSQL 17.11 custom-format dump/restore evidence.
# This lane owns only its disposable Docker containers and temporary dump.
# A local result is operational evidence only and always has release_evidence=false.

readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_CONFIGURATION=12
readonly EXIT_INCOMPLETE=13
readonly EXIT_CLEANUP=14

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_host="${WORLDSTREAM_NATIVE_PG_SOURCE_HOST:-}"
source_port="${WORLDSTREAM_NATIVE_PG_SOURCE_PORT:-}"
source_db="${WORLDSTREAM_NATIVE_PG_SOURCE_DB:-worldstream}"
source_user="${WORLDSTREAM_NATIVE_PG_SOURCE_USER:-postgres}"
target_host="${WORLDSTREAM_NATIVE_PG_TARGET_HOST:-}"
target_port="${WORLDSTREAM_NATIVE_PG_TARGET_PORT:-}"
target_db="${WORLDSTREAM_NATIVE_PG_TARGET_DB:-worldstream}"
target_user="${WORLDSTREAM_NATIVE_PG_TARGET_USER:-postgres}"
passfile="${WORLDSTREAM_NATIVE_PG_PGPASSFILE:-}"
docker_bin="${WORLDSTREAM_NATIVE_PG_DOCKER:-}"
cargo_bin="${WORLDSTREAM_NATIVE_PG_CARGO:-}"
pg_dump_bin="${WORLDSTREAM_NATIVE_PG_DUMP:-}"
pg_restore_bin="${WORLDSTREAM_NATIVE_PG_RESTORE:-}"
psql_bin="${WORLDSTREAM_NATIVE_PG_PSQL:-}"
evidence_file="${WORLDSTREAM_NATIVE_PG_EVIDENCE_FILE:-}"
temp_root=""
source_container=""
target_container=""
cleanup_status="not_started"
owned_docker=0
temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-native-pg.XXXXXX")"

cleanup() {
  local failed=0
  if [[ "$owned_docker" -eq 1 ]]; then
    [[ -z "$source_container" ]] || "$docker_bin" rm -f "$source_container" >/dev/null 2>&1 || failed=1
    [[ -z "$target_container" ]] || "$docker_bin" rm -f "$target_container" >/dev/null 2>&1 || failed=1
  fi
  if [[ -d "$temp_root" ]]; then
    rm -rf "$temp_root" || failed=1
  fi
  if [[ "$failed" -eq 0 ]]; then cleanup_status="pass"; else cleanup_status="failed"; fi
}
trap cleanup EXIT

usage() {
  printf '%s\n' \
    "Usage: scripts/postgres-native-restore-smoke.sh [--evidence PATH]" \
    "       [--source-host HOST --source-port PORT --target-host HOST --target-port PORT]" \
    "Uses digest-pinned PostgreSQL 17.11 Docker containers by default." \
    "External endpoint coordinates must already identify an isolated target."
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --evidence)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --evidence requires a path' >&2; exit "$EXIT_CONFIGURATION"; }
      evidence_file="$2"
      shift 2
      ;;
    --source-host)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --source-host requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      source_host="$2"
      shift 2
      ;;
    --source-port)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --source-port requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      source_port="$2"
      shift 2
      ;;
    --target-host)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --target-host requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      target_host="$2"
      shift 2
      ;;
    --target-port)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native PostgreSQL restore: --target-port requires a value' >&2; exit "$EXIT_CONFIGURATION"; }
      target_port="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit "$EXIT_PASS"
      ;;
    *)
      printf '%s\n' 'native PostgreSQL restore: unknown argument' >&2
      exit "$EXIT_CONFIGURATION"
      ;;
  esac
done

resolve_tool() {
  local override="$1"
  local name="$2"
  if [[ -n "$override" ]]; then
    [[ -x "$override" ]] && printf '%s' "$override"
    return 0
  fi
  command -v "$name" 2>/dev/null || true
}

docker_bin="$(resolve_tool "$docker_bin" docker)"
cargo_bin="$(resolve_tool "$cargo_bin" cargo)"
pg_dump_bin="$(resolve_tool "$pg_dump_bin" pg_dump)"
pg_restore_bin="$(resolve_tool "$pg_restore_bin" pg_restore)"
psql_bin="$(resolve_tool "$psql_bin" psql)"

write_static() {
  local status="$1" reason="$2" code="$3"
  local json
  json="$(python3 - "$status" "$reason" "$code" <<'PY'
import json, sys
print(json.dumps({
    "schema": "worldstream/native-postgres-restore-evidence/v2",
    "status": sys.argv[1],
    "reason": sys.argv[2],
    "exit_code": int(sys.argv[3]),
    "release_evidence": False,
    "target_isolated": False,
    "target_published": False,
    "secrets_emitted": False,
}, sort_keys=True, separators=(",", ":")))
PY
  )"
  if [[ -n "$evidence_file" ]]; then
    mkdir -p "$(dirname "$evidence_file")"
    printf '%s\n' "$json" >"$evidence_file"
  fi
  printf '%s\n' "$json"
}

if [[ -z "$cargo_bin" || ! -x "$cargo_bin" ]]; then
  write_static unavailable cargo_bin_unavailable "$EXIT_UNAVAILABLE"
  exit "$EXIT_UNAVAILABLE"
fi

if [[ -n "$source_host" && -n "$source_port" && -n "$target_host" && -n "$target_port" ]]; then
  for required in pg_dump_bin pg_restore_bin psql_bin; do
    if [[ -z "${!required}" || ! -x "${!required}" ]]; then
      write_static unavailable "${required}_unavailable" "$EXIT_UNAVAILABLE"
      exit "$EXIT_UNAVAILABLE"
    fi
  done
fi

if [[ -z "$source_host" || -z "$source_port" || -z "$target_host" || -z "$target_port" ]]; then
  if [[ -z "$docker_bin" || ! -x "$docker_bin" ]] || ! "$docker_bin" info >/dev/null 2>&1; then
    write_static unavailable docker_unavailable "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  source_container="worldstream-native-pg-source-$RANDOM"
  target_container="worldstream-native-pg-target-$RANDOM"
  password="worldstream_native_pg_$(date +%s)_$RANDOM"
  docker_env_file="$temp_root/docker.env"
  passfile="$temp_root/pgpass"
  umask 077
  printf 'POSTGRES_PASSWORD=%s\nPOSTGRES_DB=%s\n' "$password" "$source_db" >"$docker_env_file"
  chmod 600 "$docker_env_file"
  printf '*:*:%s:%s:%s\n' "$source_db" "$source_user" "$password" >"$passfile"
  printf '*:*:%s:%s:%s\n' "$target_db" "$target_user" "$password" >>"$passfile"
  chmod 600 "$passfile"
  if ! "$docker_bin" run --detach --rm --name "$source_container" -p 127.0.0.1::5432 \
    --env-file "$docker_env_file" \
    --mount "type=bind,source=$passfile,target=/run/secrets/worldstream-pgpass,readonly" \
    --mount "type=bind,source=$temp_root,target=/run/worldstream-native-pg" \
    postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73 >/dev/null; then
    write_static unavailable docker_container_start_failed "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  owned_docker=1
  if ! "$docker_bin" run --detach --rm --name "$target_container" -p 127.0.0.1::5432 \
    --env-file "$docker_env_file" \
    --mount "type=bind,source=$passfile,target=/run/secrets/worldstream-pgpass,readonly" \
    --mount "type=bind,source=$temp_root,target=/run/worldstream-native-pg" \
    postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73 >/dev/null; then
    "$docker_bin" rm -f "$source_container" >/dev/null 2>&1 || true
    write_static unavailable docker_container_start_failed "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  source_port="$($docker_bin port "$source_container" 5432/tcp | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p')"
  target_port="$($docker_bin port "$target_container" 5432/tcp | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p')"
  source_host="127.0.0.1"
  target_host="127.0.0.1"
  for container in "$source_container" "$target_container"; do
    for _ in $(seq 1 60); do
      if "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass "$container" \
        psql --no-password --host 127.0.0.1 --port 5432 --dbname "$source_db" \
        --username "$source_user" --quiet --command 'SELECT 1' >/dev/null 2>&1; then break; fi
      sleep 1
    done
  done
  runtime_role="worldstream_native_runtime"
  runtime_password="${password}_runtime"
  runtime_setup_sql="CREATE ROLE $runtime_role LOGIN PASSWORD '$runtime_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION; ALTER DEFAULT PRIVILEGES FOR ROLE $source_user IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO $runtime_role; ALTER DEFAULT PRIVILEGES FOR ROLE $source_user IN SCHEMA public GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO $runtime_role; GRANT USAGE ON SCHEMA public TO $runtime_role;"
  if ! "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass "$source_container" \
    psql --no-password --host 127.0.0.1 --port 5432 --dbname "$source_db" \
    --username "$source_user" --quiet --set ON_ERROR_STOP=1 --command "$runtime_setup_sql" \
    >"$temp_root/runtime-role.stdout" 2>"$temp_root/runtime-role.stderr"; then
    write_static incomplete native_source_runtime_role_setup_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  seed_stdout="$temp_root/source-seed.stdout"
  seed_stderr="$temp_root/source-seed.stderr"
  set +e
  WORLDSTREAM_PG_TRANSFER_MODE=external \
    WORLDSTREAM_PG_TRANSFER_BUILD_SOURCE=1 \
    WORLDSTREAM_PG_TRANSFER_CARGO="$cargo_bin" \
    WORLDSTREAM_PG_TRANSFER_PYTHON="$(command -v python3)" \
    WORLDSTREAM_PG_TRANSFER_ADMIN_DSN="host=$source_host port=$source_port user=$source_user password=$password dbname=$source_db" \
    WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN="host=$source_host port=$source_port user=$runtime_role password=$runtime_password dbname=$source_db" \
    WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE="$runtime_role" \
    scripts/postgres-transfer-smoke.sh --build-source >"$seed_stdout" 2>"$seed_stderr"
  seed_code=$?
  set -e
  if [[ "$seed_code" -ne 0 ]]; then
    if [[ "${WORLDSTREAM_NATIVE_PG_DEBUG:-0}" == "1" && -s "$seed_stderr" ]]; then
      sed -n '1,20p' "$seed_stderr" >&2
    fi
    write_static incomplete native_source_seed_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
  set +e
  "$cargo_bin" run --quiet --locked --manifest-path "$root_dir/crates/worldstream-postgres/Cargo.toml" \
    --bin postgres-native-prepare -- "$source_host" "$source_port" "$source_db" "$source_user" "$passfile" \
    >"$temp_root/source-prepare.stdout" 2>"$temp_root/source-prepare.stderr"
  prepare_code=$?
  set -e
  if [[ "$prepare_code" -ne 0 ]]; then
    if [[ "${WORLDSTREAM_NATIVE_PG_DEBUG:-0}" == "1" && -s "$temp_root/source-prepare.stderr" ]]; then
      sed -n '1,20p' "$temp_root/source-prepare.stderr" >&2
    fi
    write_static incomplete native_source_snapshot_prepare_failed "$EXIT_INCOMPLETE"
    exit "$EXIT_INCOMPLETE"
  fi
fi

if [[ -z "$passfile" || -L "$passfile" || ! -f "$passfile" ]]; then
  write_static incomplete pgpassfile_not_a_regular_file "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi
if [[ ! "$source_port" =~ ^[1-9][0-9]{0,4}$ || ! "$target_port" =~ ^[1-9][0-9]{0,4}$ ]]; then
  write_static incomplete invalid_endpoint_port "$EXIT_CONFIGURATION"
  exit "$EXIT_CONFIGURATION"
fi

dump_path="$temp_root/source.dump"
if [[ "$owned_docker" -eq 1 ]]; then
  make_docker_tool_wrapper() {
    local wrapper_path="$1"
    local container="$2"
    local tool="$3"
    cat >"$wrapper_path" <<EOF
#!/usr/bin/env bash
set -euo pipefail
args=()
while [[ "\$#" -gt 0 ]]; do
  case "\$1" in
    --host|--port)
      shift 2
      ;;
    "$temp_root"/*)
      args+=("/run/worldstream-native-pg/\${1#"$temp_root/"}")
      shift
      ;;
    *)
      args+=("\$1")
      shift
      ;;
  esac
done
exec "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass \\
  "$container" "$tool" --host 127.0.0.1 --port 5432 "\${args[@]}"
EOF
    chmod 700 "$wrapper_path"
  }
  make_docker_tool_wrapper "$temp_root/pg_dump" "$source_container" pg_dump
  make_docker_tool_wrapper "$temp_root/pg_restore" "$target_container" pg_restore
  cat >"$temp_root/psql" <<EOF
#!/usr/bin/env bash
set -euo pipefail
args=()
selected_container="$source_container"
selected_port=""
while [[ "\$#" -gt 0 ]]; do
  case "\$1" in
    --host)
      shift 2
      ;;
    --port)
      selected_port="\$2"
      shift 2
      ;;
    "$temp_root"/*)
      args+=("/run/worldstream-native-pg/\${1#"$temp_root/"}")
      shift
      ;;
    *)
      args+=("\$1")
      shift
      ;;
  esac
done
if [[ "\$selected_port" == "$target_port" ]]; then selected_container="$target_container"; fi
exec "$docker_bin" exec --env PGPASSFILE=/run/secrets/worldstream-pgpass \\
  "\$selected_container" psql --host 127.0.0.1 --port 5432 "\${args[@]}"
EOF
  chmod 700 "$temp_root/psql"
  pg_dump_bin="$temp_root/pg_dump"
  pg_restore_bin="$temp_root/pg_restore"
  psql_bin="$temp_root/psql"
fi
set +e
driver_stderr="$temp_root/driver.stderr"
driver_output="$($cargo_bin run --quiet --locked --manifest-path "$root_dir/crates/worldstream-postgres/Cargo.toml" \
  --bin postgres-native-restore -- "$source_host" "$source_port" "$source_db" "$source_user" \
  "$target_host" "$target_port" "$target_db" "$target_user" "$passfile" \
  "$pg_dump_bin" "$pg_restore_bin" "$psql_bin" "$dump_path" 2>"$driver_stderr")"
driver_code=$?
set -e

if [[ -z "$driver_output" ]]; then
  if [[ "${WORLDSTREAM_NATIVE_PG_DEBUG:-0}" == "1" ]]; then
    printf 'native_driver_code=%s native_driver_stderr_bytes=%s\n' "$driver_code" "$(wc -c <"$driver_stderr")" >&2
  fi
  if [[ "${WORLDSTREAM_NATIVE_PG_DEBUG:-0}" == "1" && -s "$driver_stderr" ]]; then
    sed -n '1,20p' "$driver_stderr" >&2
  fi
  write_static incomplete native_driver_failed "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi
last_line="$(printf '%s\n' "$driver_output" | tail -n 1)"
if [[ -n "$evidence_file" ]]; then
  mkdir -p "$(dirname "$evidence_file")"
  printf '%s\n' "$last_line" >"$evidence_file"
fi
printf '%s\n' "$last_line"
if [[ "$cleanup_status" == "failed" ]]; then exit "$EXIT_CLEANUP"; fi
if [[ "$driver_code" -eq 0 ]]; then exit "$EXIT_PASS"; fi
exit "$EXIT_INCOMPLETE"

#!/usr/bin/env bash
set -euo pipefail

# Disposable fail-closed PostgreSQL-backed worldstreamd smoke.
# It owns only its Docker network/container and owner-readable temporary inputs.

readonly SCHEMA="worldstream/postgresql-daemon-smoke/v1"
readonly POSTGRES_IMAGE="postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly EXPECTED_POSTGRES_DIGEST="postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_INCOMPLETE=13
readonly EXIT_CLEANUP=14
readonly RUNTIME_ROLE_ADMISSION_EXPECTED="false|false|false|false|false|false|false|false|false|false|false|false|false|false|false|false|false"

runtime_role_admission_sql() {
  cat <<'SQL'
SELECT role.rolsuper::text || '|' ||
       role.rolcreaterole::text || '|' ||
       role.rolcreatedb::text || '|' ||
       role.rolreplication::text || '|' ||
       role.rolbypassrls::text || '|' ||
       has_database_privilege(current_user, current_database(), 'CREATE')::text || '|' ||
       (EXISTS (SELECT 1 FROM pg_catalog.pg_auth_members AS membership
                WHERE membership.member = role.oid))::text || '|' ||
       has_schema_privilege(current_user, 'public', 'CREATE')::text || '|' ||
       (EXISTS (
          SELECT 1 FROM pg_catalog.pg_namespace AS namespace_row
          WHERE namespace_row.nspname = 'public' AND namespace_row.nspowner = role.oid
          UNION ALL
          SELECT 1 FROM pg_catalog.pg_class AS relation_row
          JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = relation_row.relnamespace
          WHERE namespace_row.nspname = 'public' AND relation_row.relowner = role.oid
          UNION ALL
          SELECT 1 FROM pg_catalog.pg_proc AS routine_row
          JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = routine_row.pronamespace
          WHERE namespace_row.nspname = 'public' AND routine_row.proowner = role.oid
          UNION ALL
          SELECT 1 FROM pg_catalog.pg_type AS type_row
          JOIN pg_catalog.pg_namespace AS namespace_row ON namespace_row.oid = type_row.typnamespace
          WHERE namespace_row.nspname = 'public' AND type_row.typowner = role.oid
       ))::text || '|' ||
       has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'INSERT')::text || '|' ||
       has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'UPDATE')::text || '|' ||
       has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'DELETE')::text || '|' ||
       has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'TRUNCATE')::text || '|' ||
       (EXISTS (SELECT 1 FROM unnest(ARRAY['public.worldstream_transfer_imports','public.worldstream_transfer_chunks','public.worldstream_transfer_target_fence']::text[]) AS protected_table(table_name) WHERE has_table_privilege(current_user, protected_table.table_name, 'INSERT')))::text || '|' ||
       (EXISTS (SELECT 1 FROM unnest(ARRAY['public.worldstream_transfer_imports','public.worldstream_transfer_chunks','public.worldstream_transfer_target_fence']::text[]) AS protected_table(table_name) WHERE has_table_privilege(current_user, protected_table.table_name, 'UPDATE')))::text || '|' ||
       (EXISTS (SELECT 1 FROM unnest(ARRAY['public.worldstream_transfer_imports','public.worldstream_transfer_chunks','public.worldstream_transfer_target_fence']::text[]) AS protected_table(table_name) WHERE has_table_privilege(current_user, protected_table.table_name, 'DELETE')))::text || '|' ||
       (EXISTS (SELECT 1 FROM unnest(ARRAY['public.worldstream_transfer_imports','public.worldstream_transfer_chunks','public.worldstream_transfer_target_fence']::text[]) AS protected_table(table_name) WHERE has_table_privilege(current_user, protected_table.table_name, 'TRUNCATE')))::text
FROM pg_catalog.pg_roles AS role
WHERE role.rolname = current_user
SQL
}

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

docker_bin="${WORLDSTREAM_POSTGRES_DAEMON_DOCKER:-}"
psql_bin="${WORLDSTREAM_POSTGRES_DAEMON_PSQL:-}"
ctl_bin="${WORLDSTREAM_POSTGRES_DAEMON_CTL:-$workspace_dir/target/debug/worldstreamctl}"
daemon_bin="${WORLDSTREAM_POSTGRES_DAEMON_BIN:-$workspace_dir/target/debug/worldstreamd}"
curl_bin="${WORLDSTREAM_POSTGRES_DAEMON_CURL:-}"
python_bin="${WORLDSTREAM_POSTGRES_DAEMON_PYTHON:-}"
report_file="${WORLDSTREAM_POSTGRES_DAEMON_REPORT:-}"

temp_root=""
network_name=""
postgres_name=""
postgres_started=0
daemon_pid=""
admin_password=""
runtime_password=""
postgres_digest=""
server_version_num=""
engine_identity=""
health_status="not_checked"
ready_status="not_checked"
migration_status="not_checked"
runtime_role_status="not_checked"
logs_redacted="not_checked"
cleanup_status="not_started"
status="unavailable"
reason="not_started"
exit_code="$EXIT_UNAVAILABLE"
secrets_emitted="false"

usage() {
  printf '%s\n' \
    "Usage: scripts/postgres-daemon-smoke.sh [--report PATH]" \
    "" \
    "Starts pinned disposable postgres:17.11-alpine, performs explicit worldstreamctl" \
    "admin-only migration/verification" \
    "and verifies PostgreSQL-primary worldstreamd readiness."
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --report)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'postgres daemon smoke: --report requires a path' >&2; exit "$EXIT_INCOMPLETE"; }
      report_file="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit "$EXIT_PASS"
      ;;
    *)
      printf '%s\n' 'postgres daemon smoke: unknown argument' >&2
      exit "$EXIT_INCOMPLETE"
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
psql_bin="$(resolve_tool "$psql_bin" psql)"
curl_bin="$(resolve_tool "$curl_bin" curl)"
python_bin="$(resolve_tool "$python_bin" python3)"

emit_report() {
  REPORT_SCHEMA="$SCHEMA" REPORT_STATUS="$status" REPORT_REASON="$reason" \
    REPORT_CODE="$exit_code" REPORT_CLEANUP="$cleanup_status" \
    REPORT_DIGEST="$postgres_digest" REPORT_VERSION="$server_version_num" \
    REPORT_ENGINE="$engine_identity" REPORT_HEALTH="$health_status" \
    REPORT_READY="$ready_status" REPORT_MIGRATION="$migration_status" \
    REPORT_ROLE="$runtime_role_status" REPORT_LOGS="$logs_redacted" \
    REPORT_SECRETS="$secrets_emitted" "$python_bin" - "$report_file" <<'PY'
import json
import os
import sys
from pathlib import Path

report = {
    "schema": os.environ["REPORT_SCHEMA"],
    "status": os.environ["REPORT_STATUS"],
    "reason": os.environ["REPORT_REASON"],
    "exit_code": int(os.environ["REPORT_CODE"]),
    "release_evidence": False,
    "secrets_emitted": os.environ["REPORT_SECRETS"] == "true",
    "postgres": {
        "image": "postgres:17.11-alpine",
        "digest": os.environ.get("REPORT_DIGEST") or None,
        "server_version_num": os.environ.get("REPORT_VERSION") or None,
    },
    "worldstreamd": {
        "healthz": os.environ["REPORT_HEALTH"],
        "readyz": os.environ["REPORT_READY"],
        "migration": os.environ["REPORT_MIGRATION"],
        "runtime_role": os.environ["REPORT_ROLE"],
        "engine_identity": os.environ.get("REPORT_ENGINE") or None,
    },
    "redaction": {"logs": os.environ["REPORT_LOGS"]},
    "cleanup": {"status": os.environ["REPORT_CLEANUP"]},
}
encoded = json.dumps(report, sort_keys=True, separators=(",", ":"))
if sys.argv[1]:
    path = Path(sys.argv[1])
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(encoded + "\n", encoding="utf-8")
print(encoded)
PY
}

cleanup() {
  local failed=0
  if [[ -n "$daemon_pid" ]] && kill -0 "$daemon_pid" 2>/dev/null; then
    kill -TERM "$daemon_pid" 2>/dev/null || failed=1
    for _ in $(seq 1 50); do
      kill -0 "$daemon_pid" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$daemon_pid" 2>/dev/null; then
      kill -KILL "$daemon_pid" 2>/dev/null || failed=1
    fi
    wait "$daemon_pid" 2>/dev/null || true
  fi
  if [[ "$postgres_started" -eq 1 && -n "$postgres_name" && -n "$docker_bin" ]]; then
    "$docker_bin" rm -f "$postgres_name" >/dev/null 2>&1 || failed=1
  fi
  if [[ -n "$network_name" && -n "$docker_bin" ]]; then
    "$docker_bin" network rm "$network_name" >/dev/null 2>&1 || failed=1
  fi
  if [[ -n "$temp_root" && -d "$temp_root" ]]; then
    rm -rf "$temp_root" || failed=1
  fi
  [[ "$failed" -eq 0 ]] && cleanup_status="pass" || cleanup_status="failed"
}

finish() {
  local requested_code="$1"
  trap - EXIT INT TERM
  cleanup
  if [[ "$cleanup_status" == "failed" ]]; then
    status="incomplete"
    reason="owned_resource_cleanup_failed"
    exit_code="$EXIT_CLEANUP"
  else
    exit_code="$requested_code"
  fi
  emit_report
  exit "$exit_code"
}

on_unexpected_exit() {
  local code="$1"
  [[ "$code" -eq 0 ]] && return 0
  status="unavailable"
  reason="unexpected_smoke_failure"
  finish "$code"
}

trap 'on_unexpected_exit $?' EXIT
trap 'status="incomplete"; reason="interrupted"; finish "$EXIT_INCOMPLETE"' INT TERM

if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then reason="python3_unavailable"; finish "$EXIT_UNAVAILABLE"; fi
if [[ -z "$docker_bin" || ! -x "$docker_bin" ]]; then reason="docker_unavailable"; finish "$EXIT_UNAVAILABLE"; fi
if [[ -z "$psql_bin" || ! -x "$psql_bin" ]]; then reason="psql_unavailable"; finish "$EXIT_UNAVAILABLE"; fi
if [[ ! -x "$ctl_bin" ]]; then reason="worldstreamctl_unavailable"; finish "$EXIT_UNAVAILABLE"; fi
if [[ ! -x "$daemon_bin" ]]; then reason="worldstreamd_unavailable"; finish "$EXIT_UNAVAILABLE"; fi
if [[ -z "$curl_bin" || ! -x "$curl_bin" ]]; then reason="curl_unavailable"; finish "$EXIT_UNAVAILABLE"; fi

umask 077
temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-postgres-daemon.XXXXXX")"
network_name="worldstream-postgres-daemon-$$"
postgres_name="worldstream-postgres-daemon-db-$$"
admin_password="$($python_bin -c 'import secrets; print(secrets.token_hex(24))')"
runtime_password="$($python_bin -c 'import secrets; print(secrets.token_hex(24))')"
http_port="$($python_bin -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')"

if ! "$docker_bin" network create "$network_name" >"$temp_root/network.log" 2>&1; then reason="docker_network_create_failed"; finish "$EXIT_UNAVAILABLE"; fi
if ! "$docker_bin" pull "$POSTGRES_IMAGE" >"$temp_root/postgres-pull.log" 2>&1; then reason="postgres_17_11_image_unavailable"; finish "$EXIT_UNAVAILABLE"; fi
postgres_digest="$($docker_bin image inspect "$POSTGRES_IMAGE" --format '{{index .RepoDigests 0}}' 2>/dev/null || true)"
if [[ "$postgres_digest" != "$EXPECTED_POSTGRES_DIGEST" ]]; then reason="postgres_digest_mismatch"; finish "$EXIT_UNAVAILABLE"; fi
if ! "$docker_bin" run --detach --network "$network_name" --name "$postgres_name" \
  --env POSTGRES_USER=admin --env "POSTGRES_PASSWORD=$admin_password" \
  --env POSTGRES_DB=worldstream --publish 127.0.0.1::5432 "$POSTGRES_IMAGE" \
  >"$temp_root/postgres-run.log" 2>&1; then reason="postgres_container_start_failed"; finish "$EXIT_UNAVAILABLE"; fi
postgres_started=1

for _ in $(seq 1 90); do
  postgres_port="$($docker_bin port "$postgres_name" 5432/tcp 2>/dev/null | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p' | head -n 1)"
  if [[ -n "$postgres_port" ]] && PGPASSWORD="$admin_password" "$psql_bin" \
    "host=127.0.0.1 port=$postgres_port dbname=worldstream user=admin" \
    --no-psqlrc --quiet --no-align --tuples-only --no-password -c 'SELECT 1' \
    >"$temp_root/postgres-ready.log" 2>&1 && grep -q '^1$' "$temp_root/postgres-ready.log"; then break; fi
  sleep 1
done
if [[ -z "$postgres_port" ]] || ! grep -q '^1$' "$temp_root/postgres-ready.log" 2>/dev/null; then reason="postgres_target_did_not_start"; finish "$EXIT_UNAVAILABLE"; fi

admin_psql_dsn="host=127.0.0.1 port=$postgres_port dbname=worldstream user=admin"
admin_dsn="$admin_psql_dsn password=$admin_password"
runtime_psql_dsn="host=127.0.0.1 port=$postgres_port dbname=worldstream user=runtime"
runtime_dsn="$runtime_psql_dsn password=$runtime_password"
admin_dsn_file="$temp_root/postgresql-admin-dsn"
printf '%s\n' "$admin_dsn" >"$admin_dsn_file"
chmod 600 "$admin_dsn_file"

run_admin_sql() {
  PGPASSWORD="$admin_password" "$psql_bin" "$admin_psql_dsn" --no-psqlrc --quiet \
    --no-align --tuples-only --no-password --set=ON_ERROR_STOP=1 -c "$1" \
    >"$temp_root/admin-sql.log" 2>&1
}

run_runtime_query() {
  local sql="$1"
  PGPASSWORD="$runtime_password" "$psql_bin" "$runtime_psql_dsn" --no-psqlrc --quiet \
    --no-align --tuples-only --no-password --set=ON_ERROR_STOP=1 -c "$sql"
}

server_version_num="$(PGPASSWORD="$admin_password" "$psql_bin" "$admin_psql_dsn" \
  --no-psqlrc --quiet --no-align --tuples-only --no-password -c 'SHOW server_version_num' \
  2>"$temp_root/version.log" | tr -d '[:space:]')"
if [[ "$server_version_num" != "170011" ]]; then reason="postgres_server_version_mismatch"; finish "$EXIT_INCOMPLETE"; fi

if ! run_admin_sql "CREATE ROLE runtime LOGIN PASSWORD '$runtime_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS; REVOKE CREATE ON SCHEMA public FROM PUBLIC; GRANT CONNECT ON DATABASE worldstream TO runtime; GRANT USAGE ON SCHEMA public TO runtime; ALTER DEFAULT PRIVILEGES FOR ROLE admin IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO runtime; ALTER DEFAULT PRIVILEGES FOR ROLE admin IN SCHEMA public GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO runtime"; then
  reason="runtime_role_setup_failed"
  finish "$EXIT_INCOMPLETE"
fi

adapter_log="$temp_root/admin-migration.log"
if ! "$ctl_bin" postgres migrate --dsn-file "$admin_dsn_file" >"$adapter_log" 2>&1; then
  reason="admin_migration_failed"
  finish "$EXIT_INCOMPLETE"
fi
if ! grep -Fq '"operation": "migrate"' "$adapter_log"; then
  reason="admin_migration_evidence_missing"
  finish "$EXIT_INCOMPLETE"
fi
if ! "$ctl_bin" postgres verify --dsn-file "$admin_dsn_file" \
  >>"$adapter_log" 2>&1; then
  reason="admin_schema_verification_failed"
  finish "$EXIT_INCOMPLETE"
fi
if ! grep -Fq '"operation": "verify"' "$adapter_log"; then
  reason="admin_schema_verification_evidence_missing"
  finish "$EXIT_INCOMPLETE"
fi
migration_status="pass"

if ! run_admin_sql "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO runtime; GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO runtime; REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE public.worldstream_schema_migrations, public.worldstream_transfer_imports, public.worldstream_transfer_chunks, public.worldstream_transfer_target_fence, public.worldstream_transfer_stream_imports_v2, public.worldstream_transfer_stream_chunks_v2, public.worldstream_transfer_stream_records_v2 FROM runtime"; then
  reason="runtime_privilege_hardening_failed"
  finish "$EXIT_INCOMPLETE"
fi
if ! runtime_role_admission="$(run_runtime_query "$(runtime_role_admission_sql)" 2>"$temp_root/runtime-role.log" | tr -d '[:space:]')"; then
  reason="runtime_role_admission_query_failed"
  finish "$EXIT_INCOMPLETE"
fi
if [[ "$runtime_role_admission" != "$RUNTIME_ROLE_ADMISSION_EXPECTED" ]]; then
  reason="runtime_role_not_least_privileged"
  finish "$EXIT_INCOMPLETE"
fi
runtime_role_status="pass"

secret_file="$temp_root/authority-bootstrap"
dsn_file="$temp_root/postgresql-dsn"
config_file="$temp_root/worldstreamd.toml"
data_dir="$temp_root/data"
if ! "$python_bin" - "$secret_file" <<'PY'
import secrets
import sys
from pathlib import Path
Path(sys.argv[1]).write_bytes(secrets.token_bytes(32))
PY
then
  reason="bootstrap_secret_create_failed"
  finish "$EXIT_INCOMPLETE"
fi
chmod 600 "$secret_file"
printf '%s\n' "$runtime_dsn" >"$dsn_file"
chmod 600 "$dsn_file"
if [[ "$(stat -c '%a' "$secret_file" 2>/dev/null || stat -f '%Lp' "$secret_file")" != "600" || "$(stat -c '%a' "$dsn_file" 2>/dev/null || stat -f '%Lp' "$dsn_file")" != "600" || "$(stat -c '%a' "$admin_dsn_file" 2>/dev/null || stat -f '%Lp' "$admin_dsn_file")" != "600" ]]; then
  reason="owner_only_input_permissions_failed"
  finish "$EXIT_INCOMPLETE"
fi

CONFIG_FILE="$config_file" CONFIG_BIND="127.0.0.1:$http_port" CONFIG_DATA="$data_dir" \
  CONFIG_DSN="$dsn_file" CONFIG_SECRET="$secret_file" "$python_bin" <<'PY'
import json
import os
from pathlib import Path

path = Path(os.environ["CONFIG_FILE"])
path.write_text(
    "\n".join(
        [
            "config_version = 1",
            "[server]",
            f"bind = {json.dumps(os.environ['CONFIG_BIND'])}",
            "[storage]",
            'profile = "postgres-primary"',
            f"data_dir = {json.dumps(os.environ['CONFIG_DATA'])}",
            "[storage.postgresql]",
            f"dsn_file = {json.dumps(os.environ['CONFIG_DSN'])}",
            "[authority.bootstrap]",
            f"secret_file = {json.dumps(os.environ['CONFIG_SECRET'])}",
            "",
        ]
    ),
    encoding="utf-8",
)
PY

daemon_log="$temp_root/worldstreamd.log"
"$daemon_bin" --config "$config_file" >"$daemon_log" 2>&1 &
daemon_pid="$!"
if ! kill -0 "$daemon_pid" 2>/dev/null; then
  reason="worldstreamd_start_failed"
  finish "$EXIT_INCOMPLETE"
fi

probe() {
  local path="$1"
  local output="$2"
  local code
  code="$($curl_bin -sS --connect-timeout 1 --max-time 3 -o "$output" -w '%{http_code}' "http://127.0.0.1:$http_port$path" 2>"$temp_root/curl.log" || true)"
  [[ "$code" == "200" ]]
}

for _ in $(seq 1 90); do
  if probe /healthz "$temp_root/health.json"; then break; fi
  if ! kill -0 "$daemon_pid" 2>/dev/null; then
    reason="worldstreamd_exited_before_ready"
    finish "$EXIT_INCOMPLETE"
  fi
  sleep 1
done
if ! probe /healthz "$temp_root/health.json" || ! probe /readyz "$temp_root/ready.json" || ! probe /version "$temp_root/version.json"; then
  reason="worldstreamd_readiness_probe_failed"
  finish "$EXIT_INCOMPLETE"
fi

if ! "$python_bin" "$temp_root/health.json" "$temp_root/ready.json" "$temp_root/version.json" <<'PY'
import json
import sys

health, ready, version = (json.load(open(path, encoding="utf-8")) for path in sys.argv[1:])
if health != {"status": "ok"}:
    raise SystemExit("healthz contract")
if ready != {"status": "ready"}:
    raise SystemExit("readyz contract")
engine = version.get("engine")
if not isinstance(engine, dict) or engine.get("profile") != "postgres-primary":
    raise SystemExit("engine profile contract")
if engine.get("status") != "verified":
    raise SystemExit("engine status contract")
if engine.get("exact_identity") != "postgresql/17.11; server_version_num=170011":
    raise SystemExit("engine identity contract")
lower = json.dumps((ready, version), sort_keys=True).lower()
if "degraded" in lower or "fallback" in lower:
    raise SystemExit("degraded or fallback state")
PY
then
  reason="worldstreamd_response_contract_failed"
  finish "$EXIT_INCOMPLETE"
fi
health_status="pass"
ready_status="pass"
engine_identity="postgresql/17.11; server_version_num=170011"

if ! ADMIN_PASSWORD="$admin_password" RUNTIME_PASSWORD="$runtime_password" SECRET_FILE="$secret_file" DAEMON_LOG="$daemon_log" "$python_bin" <<'PY'
import os
from pathlib import Path

log = Path(os.environ["DAEMON_LOG"]).read_bytes()
secret = Path(os.environ["SECRET_FILE"]).read_bytes()
for value in (os.environ["ADMIN_PASSWORD"].encode(), os.environ["RUNTIME_PASSWORD"].encode(), secret):
    if value and value in log:
        raise SystemExit("credential material found in daemon log")
PY
then
  secrets_emitted="true"
  reason="secret_material_in_daemon_log"
  finish "$EXIT_INCOMPLETE"
fi
logs_redacted="pass"
status="pass"
reason="verified_postgresql_daemon_ready"
finish "$EXIT_PASS"

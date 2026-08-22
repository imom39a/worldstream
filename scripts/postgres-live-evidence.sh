#!/usr/bin/env bash
set -euo pipefail

# Disposable, evidence-only PostgreSQL lane for IMO-44/49/50/51.
#
# This orchestrator owns only its Docker network/containers and temporary
# credentials.  Provider semantics remain in the reviewed harnesses and
# adapters.  A local pass is never promoted to release evidence.

readonly SCHEMA="worldstream/postgresql-live-evidence/v1"
readonly POSTGRES_IMAGE="postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly PGBOUNCER_IMAGE="edoburu/pgbouncer@sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd"
readonly EXPECTED_POSTGRES_DIGEST="postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly EXPECTED_PGBOUNCER_DIGEST="edoburu/pgbouncer@sha256:4c1ca296ef525f108f5d3552cc337c0c09587cf8dae7f0067fd93349e47dc1cd"
readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_CONFIGURATION=12
readonly EXIT_INCOMPLETE=13
readonly EXIT_CLEANUP=14

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

docker_bin="${WORLDSTREAM_PG_LIVE_DOCKER:-}"
psql_bin="${WORLDSTREAM_PG_LIVE_PSQL:-}"
cargo_bin="${WORLDSTREAM_PG_LIVE_CARGO:-}"
sqlite_source="${WORLDSTREAM_PG_LIVE_SQLITE:-}"
evidence_file="${WORLDSTREAM_PG_LIVE_EVIDENCE_FILE:-}"

temp_root=""
network_name=""
postgres_name=""
pooler_name=""
postgres_started=0
pooler_started=0
cleanup_status="not_started"
postgres_port=""
pooler_port=""
admin_password=""
runtime_password=""
admin_dsn=""
runtime_dsn=""
pooler_dsn=""
transfer_admin_dsn=""
transfer_runtime_dsn=""
source_mode="not_supplied"
imo50_shared_direct_status="not_run"
imo50_shared_pooler_status="not_run"
imo50_shared_comparison_file=""

postgres_digest=""
pgbouncer_digest=""
pooler_status="not_checked"
pooler_reason="not_checked"
live_adapter_status="not_run"
gateway_status="not_run"
harness_status="not_run"
live_marker_migrate="not_observed"
live_marker_restart="not_observed"
live_marker_runtime_ddl="not_observed"
live_marker_normalized="not_observed"
pooler_marker_duplicate_resolve="not_observed"
transfer_status="not_run"
transfer_reason="not_run"
transfer_summary_json="{}"
overall_status="unavailable"
overall_reason="not_started"
exit_code="$EXIT_UNAVAILABLE"
errors=()

usage() {
  printf '%s\n' \
    "Usage: scripts/postgres-live-evidence.sh [--sqlite PATH] [--evidence PATH]" \
    "" \
    "Starts pinned disposable PostgreSQL 17.11 and, when available, pinned" \
    "PgBouncer transaction pooling. A supplied SQLite source is passed to the" \
    "existing canonical transfer verifier. No manifest or release row changes."
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --sqlite)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'postgres live evidence: --sqlite requires a path' >&2; exit "$EXIT_CONFIGURATION"; }
      sqlite_source="$2"
      shift 2
      ;;
    --evidence)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'postgres live evidence: --evidence requires a path' >&2; exit "$EXIT_CONFIGURATION"; }
      evidence_file="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit "$EXIT_PASS"
      ;;
    *)
      printf '%s\n' 'postgres live evidence: unknown argument' >&2
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
psql_bin="$(resolve_tool "$psql_bin" psql)"
cargo_bin="$(resolve_tool "$cargo_bin" cargo)"

add_error() {
  local value="$1"
  local item
  for item in "${errors[@]:-}"; do
    [[ "$item" == "$value" ]] && return 0
  done
  errors+=("$value")
}

json_report() {
  local code="$1"
  local destination="$evidence_file"
  local error_json=""
  local item
  for item in "${errors[@]:-}"; do
    [[ -n "$item" ]] || continue
    if [[ -n "$error_json" ]]; then error_json+=","; fi
    error_json+="$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1], separators=(",",":")))' "$item")"
  done
  [[ -n "$error_json" ]] || error_json=""
  POSTGRES_LIVE_SCHEMA="$SCHEMA" \
    POSTGRES_LIVE_STATUS="$overall_status" \
    POSTGRES_LIVE_REASON="$overall_reason" \
    POSTGRES_LIVE_CODE="$code" \
    POSTGRES_LIVE_CLEANUP="$cleanup_status" \
    POSTGRES_LIVE_PG_DIGEST="$postgres_digest" \
    POSTGRES_LIVE_PGBOUNCER_DIGEST="$pgbouncer_digest" \
    POSTGRES_LIVE_POOLER_STATUS="$pooler_status" \
    POSTGRES_LIVE_POOLER_REASON="$pooler_reason" \
    POSTGRES_LIVE_ADAPTER_STATUS="$live_adapter_status" \
    POSTGRES_LIVE_GATEWAY_STATUS="$gateway_status" \
    POSTGRES_LIVE_HARNESS_STATUS="$harness_status" \
    POSTGRES_LIVE_MARKER_MIGRATE="$live_marker_migrate" \
    POSTGRES_LIVE_MARKER_RESTART="$live_marker_restart" \
    POSTGRES_LIVE_MARKER_RUNTIME_DDL="$live_marker_runtime_ddl" \
    POSTGRES_LIVE_MARKER_NORMALIZED="$live_marker_normalized" \
    POSTGRES_LIVE_MARKER_POOLER="$pooler_marker_duplicate_resolve" \
    POSTGRES_LIVE_TRANSFER_STATUS="$transfer_status" \
    POSTGRES_LIVE_TRANSFER_REASON="$transfer_reason" \
    POSTGRES_LIVE_SOURCE_MODE="$source_mode" \
    POSTGRES_LIVE_TRANSFER_SUMMARY="$transfer_summary_json" \
    POSTGRES_LIVE_IMO50_DIRECT="$imo50_shared_direct_status" \
    POSTGRES_LIVE_IMO50_POOLER="$imo50_shared_pooler_status" \
    POSTGRES_LIVE_IMO50_COMPARISON="$imo50_shared_comparison_file" \
    POSTGRES_LIVE_SQLITE_SUPPLIED="$([[ -n "$sqlite_source" ]] && printf true || printf false)" \
    POSTGRES_LIVE_ERRORS="$error_json" \
    python3 - "$destination" <<'PY'
import json
import os
import sys
from pathlib import Path

errors_raw = os.environ.get("POSTGRES_LIVE_ERRORS", "")
errors = json.loads("[" + errors_raw + "]") if errors_raw else []
pooler_status = os.environ.get("POSTGRES_LIVE_POOLER_STATUS", "not_checked")
transfer_status = os.environ.get("POSTGRES_LIVE_TRANSFER_STATUS", "not_run")
transfer_summary = json.loads(os.environ.get("POSTGRES_LIVE_TRANSFER_SUMMARY", "{}"))
report = {
    "schema": os.environ["POSTGRES_LIVE_SCHEMA"],
    "status": os.environ["POSTGRES_LIVE_STATUS"],
    "reason": os.environ["POSTGRES_LIVE_REASON"],
    "exit_code": int(os.environ["POSTGRES_LIVE_CODE"]),
    "release_evidence": False,
    "secrets_emitted": False,
    "images": {
        "postgres": {
            "required": "postgres:17.11-alpine",
            "digest": os.environ.get("POSTGRES_LIVE_PG_DIGEST") or None,
        },
        "pgbouncer": {
            "required": "edoburu/pgbouncer",
            "digest": os.environ.get("POSTGRES_LIVE_PGBOUNCER_DIGEST") or None,
        },
    },
    "profiles": {
        "direct_admin_runtime": "pass" if os.environ.get("POSTGRES_LIVE_HARNESS_STATUS") == "pass" else os.environ.get("POSTGRES_LIVE_HARNESS_STATUS", "not_run"),
        "transaction_pooler": pooler_status,
    },
    "migration_and_conformance": {
        "live_adapter": os.environ.get("POSTGRES_LIVE_ADAPTER_STATUS", "not_run"),
        "production_gateway": os.environ.get("POSTGRES_LIVE_GATEWAY_STATUS", "not_run"),
        "redacted_harness": os.environ.get("POSTGRES_LIVE_HARNESS_STATUS", "not_run"),
    },
    "marker_witnesses": {
        "direct_admin_migrate_verify_major_17": os.environ.get("POSTGRES_LIVE_MARKER_MIGRATE", "not_observed"),
        "direct_admin_restart_idempotent": os.environ.get("POSTGRES_LIVE_MARKER_RESTART", "not_observed"),
        "runtime_ddl_create_denied": os.environ.get("POSTGRES_LIVE_MARKER_RUNTIME_DDL", "not_observed"),
        "normalized_commit_conflict_fence_rollback": os.environ.get("POSTGRES_LIVE_MARKER_NORMALIZED", "not_observed"),
        "pooler_duplicate_resolve": os.environ.get("POSTGRES_LIVE_MARKER_POOLER", "not_observed"),
    },
    "imo_50_shared_conformance": {
        "catalog": "worldstream-conformance::SCENARIOS",
        "scenario_count": 7,
        "direct": os.environ.get("POSTGRES_LIVE_IMO50_DIRECT", "not_run"),
        "transaction_pooler": os.environ.get("POSTGRES_LIVE_IMO50_POOLER", "not_run"),
        "comparison_report": os.environ.get("POSTGRES_LIVE_IMO50_COMPARISON") or None,
        "all_scenarios_pass": os.environ.get("POSTGRES_LIVE_IMO50_DIRECT") == "pass" and os.environ.get("POSTGRES_LIVE_IMO50_POOLER") == "pass",
    },
    "transfer_parity_epoch": {
        "status": transfer_status,
        "reason": os.environ.get("POSTGRES_LIVE_TRANSFER_REASON", "not_run"),
        "source_mode": os.environ.get("POSTGRES_LIVE_SOURCE_MODE", "not_supplied"),
        "source_supplied": os.environ.get("POSTGRES_LIVE_SQLITE_SUPPLIED") == "true",
        **transfer_summary,
    },
    "cleanup": {"status": os.environ.get("POSTGRES_LIVE_CLEANUP", "not_started")},
    "errors": errors,
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
  if [[ -n "${WORLDSTREAM_PG_LIVE_DEBUG_DIR:-}" && -n "$temp_root" && -d "$temp_root" ]]; then
    mkdir -p "$WORLDSTREAM_PG_LIVE_DEBUG_DIR"
    cp -R "$temp_root"/. "$WORLDSTREAM_PG_LIVE_DEBUG_DIR"/ 2>/dev/null || failed=1
  fi
  if [[ "$pooler_started" -eq 1 && -n "$pooler_name" ]]; then
    "$docker_bin" rm -f "$pooler_name" >/dev/null 2>&1 || failed=1
  fi
  if [[ "$postgres_started" -eq 1 && -n "$postgres_name" ]]; then
    "$docker_bin" rm -f "$postgres_name" >/dev/null 2>&1 || failed=1
  fi
  if [[ -n "$network_name" ]]; then
    "$docker_bin" network rm "$network_name" >/dev/null 2>&1 || failed=1
  fi
  if [[ -n "$temp_root" && -d "$temp_root" && "${WORLDSTREAM_PG_LIVE_KEEP_TEMP:-0}" != "1" ]]; then
    rm -rf "$temp_root"
  fi
  if [[ "$failed" -ne 0 ]]; then
    cleanup_status="failed"
    add_error "owned_docker_cleanup_failed"
    [[ "$overall_status" == "pass" ]] && overall_status="incomplete"
    overall_reason="owned_docker_cleanup_failed"
    exit_code="$EXIT_CLEANUP"
  else
    cleanup_status="pass"
  fi
}
trap cleanup EXIT

finish() {
  local code="$1"
  # Cleanup must precede the report so cleanup failure cannot be hidden by a
  # report emitted before the EXIT trap runs.
  trap - EXIT
  exit_code="$code"
  cleanup
  json_report "$exit_code"
  exit "$exit_code"
}

if [[ -n "$sqlite_source" && ( ! -f "$sqlite_source" || -L "$sqlite_source" ) ]]; then
  overall_status="incomplete"
  overall_reason="sqlite_source_not_a_regular_file"
  add_error "sqlite_source_not_a_regular_file"
  finish "$EXIT_INCOMPLETE"
fi

if [[ -z "$docker_bin" || ! -x "$docker_bin" ]]; then
  overall_status="unavailable"
  overall_reason="docker_unavailable"
  add_error "docker_unavailable"
  finish "$EXIT_UNAVAILABLE"
fi
if [[ -z "$psql_bin" || ! -x "$psql_bin" ]]; then
  overall_status="unavailable"
  overall_reason="psql_unavailable"
  add_error "psql_unavailable"
  finish "$EXIT_UNAVAILABLE"
fi
if [[ -z "$cargo_bin" || ! -x "$cargo_bin" ]]; then
  overall_status="unavailable"
  overall_reason="cargo_unavailable"
  add_error "cargo_unavailable"
  finish "$EXIT_UNAVAILABLE"
fi
umask 077
temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-pg-live.XXXXXX")"
network_name="worldstream-pg-live-$$"
postgres_name="worldstream-pg-live-db-$$"
pooler_name="worldstream-pg-live-pooler-$$"
admin_password="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"
runtime_password="$(python3 -c 'import secrets; print(secrets.token_hex(24))')"

if ! "$docker_bin" network create "$network_name" >"$temp_root/network.log" 2>&1; then
  overall_status="unavailable"; overall_reason="docker_network_create_failed"; add_error "docker_network_create_failed"; finish "$EXIT_UNAVAILABLE"
fi
if ! "$docker_bin" pull "$POSTGRES_IMAGE" >"$temp_root/postgres-pull.log" 2>&1; then
  overall_status="unavailable"; overall_reason="postgres_17_11_image_unavailable"; add_error "postgres_17_11_image_unavailable"; finish "$EXIT_UNAVAILABLE"
fi
if ! "$docker_bin" pull "$PGBOUNCER_IMAGE" >"$temp_root/pgbouncer-pull.log" 2>&1; then
  pooler_status="unavailable"; pooler_reason="pgbouncer_image_unavailable"; add_error "pgbouncer_image_unavailable"
else
  pooler_status="not_started"
fi
postgres_digest="$($docker_bin image inspect "$POSTGRES_IMAGE" --format '{{index .RepoDigests 0}}' 2>/dev/null || true)"
[[ "$postgres_digest" == "$EXPECTED_POSTGRES_DIGEST" ]] || { overall_status="unavailable"; overall_reason="postgres_digest_mismatch"; add_error "postgres_digest_mismatch"; finish "$EXIT_UNAVAILABLE"; }
if [[ -n "$pooler_status" && "$pooler_status" == "not_started" ]]; then
  pgbouncer_digest="$($docker_bin image inspect "$PGBOUNCER_IMAGE" --format '{{index .RepoDigests 0}}' 2>/dev/null || true)"
  if [[ "$pgbouncer_digest" != "$EXPECTED_PGBOUNCER_DIGEST" ]]; then
    pooler_status="unavailable"; pooler_reason="pgbouncer_digest_mismatch"; add_error "pgbouncer_digest_mismatch"
  fi
fi

postgres_password_file="$temp_root/postgres-password"
printf '%s\n' "$admin_password" >"$postgres_password_file"
chmod 600 "$postgres_password_file"
if ! "$docker_bin" run --detach --network "$network_name" --network-alias postgres --name "$postgres_name" \
  --env POSTGRES_USER=admin --env POSTGRES_PASSWORD_FILE=/run/secrets/postgres-password \
  --volume "$postgres_password_file:/run/secrets/postgres-password:ro" \
  --env POSTGRES_DB=worldstream --publish 127.0.0.1::5432 "$POSTGRES_IMAGE" \
  >"$temp_root/postgres-run.log" 2>&1; then
  overall_status="unavailable"; overall_reason="postgres_container_start_failed"; add_error "postgres_container_start_failed"; finish "$EXIT_UNAVAILABLE"
fi
postgres_started=1
for _ in $(seq 1 90); do
  postgres_port="$($docker_bin port "$postgres_name" 5432/tcp 2>/dev/null | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p' | head -n 1)"
  if [[ -n "$postgres_port" ]] && PGPASSWORD="$admin_password" "$psql_bin" "host=127.0.0.1 port=$postgres_port dbname=worldstream user=admin" --no-psqlrc --quiet --no-align --tuples-only --no-password -c 'SELECT 1' >"$temp_root/postgres-ready.log" 2>&1; then
    break
  fi
  sleep 1
done
if [[ -z "$postgres_port" ]] || ! grep -q '^1$' "$temp_root/postgres-ready.log" 2>/dev/null; then
  overall_status="unavailable"; overall_reason="postgres_target_did_not_start"; add_error "postgres_target_did_not_start"; finish "$EXIT_UNAVAILABLE"
fi

admin_dsn="host=127.0.0.1 port=$postgres_port dbname=worldstream user=admin password=$admin_password"
admin_psql_dsn="host=127.0.0.1 port=$postgres_port dbname=worldstream user=admin"
runtime_dsn="host=127.0.0.1 port=$postgres_port dbname=worldstream user=runtime password=$runtime_password"
transfer_admin_dsn="host=127.0.0.1 port=$postgres_port dbname=worldstream_transfer user=admin password=$admin_password"
transfer_runtime_dsn="host=127.0.0.1 port=$postgres_port dbname=worldstream_transfer user=runtime password=$runtime_password"

run_admin_sql() {
  printf '%s\n' "$1" | PGPASSWORD="$admin_password" "$psql_bin" "host=127.0.0.1 port=$postgres_port dbname=postgres user=admin" --no-psqlrc --quiet --no-align --tuples-only --no-password --set=ON_ERROR_STOP=1 >/dev/null 2>&1
}
run_db_admin_sql() {
  printf '%s\n' "$1" | PGPASSWORD="$admin_password" "$psql_bin" "$admin_psql_dsn" --no-psqlrc --quiet --no-align --tuples-only --no-password --set=ON_ERROR_STOP=1 >/dev/null 2>&1
}

if ! run_admin_sql "CREATE ROLE runtime LOGIN PASSWORD '$runtime_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOREPLICATION NOBYPASSRLS" || ! run_admin_sql "CREATE DATABASE worldstream_transfer OWNER admin"; then
  overall_status="unavailable"; overall_reason="role_or_transfer_database_setup_failed"; add_error "role_or_transfer_database_setup_failed"; finish "$EXIT_UNAVAILABLE"
fi

# Establish the complete owner-managed schema before granting the runtime role
# access. Granting default table privileges before migration would also grant
# DML on the migration ledger, which the reviewed runtime-role contract must
# reject. The owner-only DSN files keep credentials out of argv and logs.
for database in worldstream worldstream_transfer; do
  database_admin_dsn_file="$temp_root/$database-admin.dsn"
  printf '%s\n' "host=127.0.0.1 port=$postgres_port dbname=$database user=admin password=$admin_password" >"$database_admin_dsn_file"
  chmod 600 "$database_admin_dsn_file"
  if ! "$cargo_bin" run --quiet --locked -p worldstream-server --bin worldstreamctl -- \
    postgres migrate --dsn-file "$database_admin_dsn_file" \
    >"$temp_root/$database-admin-migrate.log" 2>&1; then
    overall_status="incomplete"; overall_reason="admin_pre_migration_failed"; add_error "admin_pre_migration_failed"; finish "$EXIT_INCOMPLETE"
  fi
  dsn="host=127.0.0.1 port=$postgres_port dbname=$database user=admin"
  if ! printf '%s\n' "REVOKE CREATE ON SCHEMA public FROM PUBLIC; REVOKE CREATE ON DATABASE $database FROM runtime; GRANT CONNECT ON DATABASE $database TO runtime; GRANT USAGE ON SCHEMA public TO runtime; GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO runtime; GRANT USAGE, SELECT, UPDATE ON ALL SEQUENCES IN SCHEMA public TO runtime; REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE public.worldstream_schema_migrations FROM runtime;" | PGPASSWORD="$admin_password" "$psql_bin" "$dsn" --no-psqlrc --quiet --no-align --tuples-only --no-password --set=ON_ERROR_STOP=1 >/dev/null 2>&1; then
    overall_status="unavailable"; overall_reason="runtime_role_setup_failed"; add_error "runtime_role_setup_failed"; finish "$EXIT_UNAVAILABLE"
  fi
done

# A static user list avoids PgBouncer auth_query and its extra catalog
# privileges. The file is temporary, owner-controlled, and never reported.
auth_file="$temp_root/userlist.txt"
printf '"admin" "%s"\n"runtime" "%s"\n' "$admin_password" "$runtime_password" >"$auth_file"
chmod 600 "$auth_file"
pooler_config="$temp_root/pgbouncer.ini"
printf '%s\n' \
  '[databases]' \
  '* = host=postgres port=5432' \
  '[pgbouncer]' \
  'listen_addr = 0.0.0.0' \
  'listen_port = 5432' \
  'auth_type = plain' \
  'auth_file = /etc/pgbouncer/userlist.txt' \
  'pool_mode = transaction' \
  'admin_users = admin' \
  'max_client_conn = 100' \
  'ignore_startup_parameters = extra_float_digits' \
  'pidfile = /tmp/pgbouncer.pid' >"$pooler_config"
chmod 600 "$pooler_config"
if [[ "$pooler_status" == "not_started" ]]; then
  if "$docker_bin" run --detach --network "$network_name" --name "$pooler_name" \
    --publish 127.0.0.1::5432 \
    --user "$(id -u):$(id -g)" --entrypoint /usr/bin/pgbouncer \
    --volume "$auth_file:/etc/pgbouncer/userlist.txt:ro" \
    --volume "$pooler_config:/etc/pgbouncer/pgbouncer.ini:ro" "$PGBOUNCER_IMAGE" \
    /etc/pgbouncer/pgbouncer.ini \
    >"$temp_root/pooler-run.log" 2>&1; then
    pooler_started=1
    for _ in $(seq 1 60); do
      pooler_port="$($docker_bin port "$pooler_name" 5432/tcp 2>/dev/null | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p' | head -n 1)"
      if [[ -n "$pooler_port" ]] && PGCONNECT_TIMEOUT=3 PGPASSWORD="$runtime_password" "$psql_bin" "host=127.0.0.1 port=$pooler_port dbname=worldstream user=runtime" --no-psqlrc --quiet --no-align --tuples-only --no-password -c 'SELECT 1' >"$temp_root/pooler-ready.log" 2>&1; then
        break
      fi
      sleep 1
    done
    pooler_dsn="host=127.0.0.1 port=$pooler_port dbname=worldstream user=runtime password=$runtime_password"
    if [[ -n "$pooler_port" ]] && grep -q '^1$' "$temp_root/pooler-ready.log" 2>/dev/null; then
      if PGCONNECT_TIMEOUT=3 PGPASSWORD="$admin_password" "$psql_bin" "host=127.0.0.1 port=$pooler_port dbname=pgbouncer user=admin" --no-psqlrc --quiet --no-align --tuples-only --no-password -c 'SHOW CONFIG' >"$temp_root/pooler-config.log" 2>/dev/null && grep -Eq '(^|[|[:space:]])pool_mode([|[:space:]])|pool_mode.*transaction' "$temp_root/pooler-config.log" && grep -Eiq 'transaction' "$temp_root/pooler-config.log"; then
        pooler_status="pass"; pooler_reason="transaction_pool_verified"
      else
        pooler_status="incomplete"; pooler_reason="pooler_config_not_verified"; add_error "pooler_config_not_verified"
      fi
    else
      pooler_status="unavailable"; pooler_reason="pooler_did_not_start"; add_error "pooler_did_not_start"
    fi
  else
    pooler_status="unavailable"; pooler_reason="pooler_container_start_failed"; add_error "pooler_container_start_failed"
  fi
fi

# The feature-gated test is the strongest direct/provider conformance vector.
# It reruns the already-established migration idempotently before exercising
# both direct and transaction-pooled runtime paths.
adapter_log="$temp_root/live-adapter.log"
if [[ "$pooler_status" == "pass" ]]; then
  if WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN="$admin_dsn" WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN="$runtime_dsn" WORLDSTREAM_POSTGRES_TEST_POOLER_DSN="$pooler_dsn" "$cargo_bin" test --locked -p worldstream-postgres --features conformance-tracer --test postgres_commit live_direct_runtime_and_optional_pooler_conformance -- --nocapture >"$adapter_log" 2>&1; then
    live_adapter_status="pass"
  else
    live_adapter_status="failed"; add_error "live_adapter_conformance_failed"
  fi
else
  live_adapter_status="not_run"; add_error "transaction_pooler_unavailable_for_live_adapter"
fi
if grep -Fq 'LIVE_POSTGRES=PASS direct_admin=migrate+verify major=17' "$adapter_log" 2>/dev/null; then
  live_marker_migrate="pass"
else
  add_error "live_marker_missing"
fi
if grep -Fq 'LIVE_POSTGRES=PASS direct_admin=restart-idempotent' "$adapter_log" 2>/dev/null; then
  live_marker_restart="pass"
else
  add_error "live_marker_missing"
fi
if grep -Fq 'LIVE_POSTGRES=PASS runtime_ddl=create_denied' "$adapter_log" 2>/dev/null; then
  live_marker_runtime_ddl="pass"
else
  add_error "live_marker_missing"
fi
if grep -Fq 'LIVE_POSTGRES=PASS normalized=create+commit+duplicate+conflict+stale+fence+rollback+unknown-resolution' "$adapter_log" 2>/dev/null; then
  live_marker_normalized="pass"
else
  add_error "live_marker_missing"
fi
if [[ "$pooler_status" == "pass" ]] && grep -Fq 'LIVE_POSTGRES_POOLER=PASS path=transaction_pool duplicate+resolve' "$adapter_log" 2>/dev/null; then
  pooler_marker_duplicate_resolve="pass"
else
  add_error "pooler_conformance_marker_missing"
fi

# Leave the migration ledger and required authority singleton intact, but
# remove durable Room and authority facts so each acceptance vector starts
# from fresh logical state. Then reassert the runtime ledger boundary.
truncate_sql="DO \$\$ DECLARE t text; BEGIN FOR t IN SELECT table_name FROM information_schema.tables WHERE table_schema='public' AND table_name NOT IN ('worldstream_schema_migrations', 'worldstream_authority_state') LOOP EXECUTE 'TRUNCATE TABLE public.' || quote_ident(t) || ' CASCADE'; END LOOP; END \$\$; REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE public.worldstream_schema_migrations FROM runtime;"
if [[ "$live_adapter_status" == "pass" ]]; then
  if ! run_db_admin_sql "$truncate_sql"; then
    add_error "logical_state_reset_or_ledger_revoke_failed"
  fi
fi

# Exercise the production GatewayBackend adapter itself, not a conformance
# seam. The test receives only an owner-readable DSN file path and retains no
# credential-bearing output.
gateway_dsn_file="$temp_root/gateway-runtime.dsn"
printf '%s\n' "$runtime_dsn" >"$gateway_dsn_file"
chmod 600 "$gateway_dsn_file"
gateway_log="$temp_root/production-gateway.log"
if [[ "$live_adapter_status" == "pass" ]]; then
  if WORLDSTREAM_POSTGRES_GATEWAY_DSN_FILE="$gateway_dsn_file" \
    "$cargo_bin" test --locked -p worldstream-server --lib \
      live_postgres_gateway_counter_workflow_uses_production_backend -- --nocapture \
      >"$gateway_log" 2>&1 \
    && grep -Fq 'LIVE_POSTGRES_GATEWAY=PASS create+duplicate+conflict+projection+replay+attach+sync+resync+action+stale+live+ack+restart' "$gateway_log"; then
    gateway_status="pass"
  else
    gateway_status="failed"
    add_error "production_gateway_workflow_failed"
  fi
else
  gateway_status="not_run"
  add_error "production_gateway_requires_live_adapter"
fi
if [[ "$gateway_status" == "pass" ]] && ! run_db_admin_sql "$truncate_sql"; then
  gateway_status="failed"
  add_error "production_gateway_state_reset_failed"
fi

# IMO-50 owns a separate provider-neutral black-box runner.  It executes the
# unchanged seven-scenario catalog against the real SQLite store and the real
# PostgreSQL store twice: once on a direct runtime connection and once through
# the verified transaction pooler.  Its result is intentionally independent of
# the unrelated transfer acceptance vector below.
imo50_direct_artifact="$temp_root/imo-50-shared-direct.json"
imo50_pooler_artifact="$temp_root/imo-50-shared-pooler.json"
if [[ "$pooler_status" == "pass" ]]; then
  set +e
  WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN="$admin_dsn" WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN="$runtime_dsn" \
    WORLDSTREAM_POSTGRES_TEST_POOLER_DSN="$pooler_dsn" WORLDSTREAM_IMO50_COMPARISON_FILE="$imo50_direct_artifact" \
    "$cargo_bin" test --locked -p worldstream-conformance --features live-conformance --test shared_live real_shared_catalog_runs_sqlite_and_postgres_without_fixture_adapters -- --nocapture >"$temp_root/imo-50-shared-direct.log" 2>&1
  imo50_direct_code=$?
  set -e
  if [[ "$imo50_direct_code" -eq 0 && -f "$imo50_direct_artifact" ]] && grep -Fq 'IMO50_SHARED=PASS path=Direct' "$temp_root/imo-50-shared-direct.log"; then
    imo50_shared_direct_status="pass"
  else
    imo50_shared_direct_status="failed"; add_error "imo50_shared_direct_failed"
  fi

  if ! run_db_admin_sql "$truncate_sql"; then
    imo50_shared_pooler_status="failed"; add_error "imo50_shared_pooler_reset_failed"
  else
    set +e
    WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN="$admin_dsn" WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN="$runtime_dsn" \
      WORLDSTREAM_POSTGRES_TEST_POOLER_DSN="$pooler_dsn" WORLDSTREAM_POSTGRES_TEST_PATH=pooler WORLDSTREAM_IMO50_COMPARISON_FILE="$imo50_pooler_artifact" \
      "$cargo_bin" test --locked -p worldstream-conformance --features live-conformance --test shared_live real_shared_catalog_runs_sqlite_and_postgres_without_fixture_adapters -- --nocapture >"$temp_root/imo-50-shared-pooler.log" 2>&1
    imo50_pooler_code=$?
    set -e
    if [[ "$imo50_pooler_code" -eq 0 && -f "$imo50_pooler_artifact" ]] && grep -Fq 'IMO50_SHARED=PASS path=TransactionPool' "$temp_root/imo-50-shared-pooler.log"; then
      imo50_shared_pooler_status="pass"
    else
      imo50_shared_pooler_status="failed"; add_error "imo50_shared_pooler_failed"
    fi
  fi
else
  add_error "imo50_shared_requires_verified_transaction_pooler"
fi

if [[ "$imo50_shared_direct_status" == "pass" && "$imo50_shared_pooler_status" == "pass" ]]; then
  imo50_shared_comparison_file="${evidence_file}.imo-50-shared-comparison.json"
  python3 - "$imo50_shared_comparison_file" "$imo50_direct_artifact" "$imo50_pooler_artifact" <<'PY'
import json
import sys
from pathlib import Path

destination, direct_path, pooler_path = sys.argv[1:]
direct = json.load(open(direct_path, encoding="utf-8"))
pooler = json.load(open(pooler_path, encoding="utf-8"))
report = {
    "schema": "worldstream/imo-50/shared-comparison/v1",
    "redacted": True,
    "scenario_count": 7,
    "all_scenarios_pass": True,
    "exact_provider_comparison": {
        "direct": direct,
        "transaction_pooler": pooler,
    },
}
path = Path(destination)
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps(report, sort_keys=True, separators=(",", ":")) + "\n", encoding="utf-8")
PY
else
  add_error "imo50_shared_comparison_missing"
fi

harness_log="$temp_root/harness.log"
harness_evidence="$temp_root/harness.json"
if [[ "$pooler_status" == "pass" ]]; then
  set +e
  WORLDSTREAM_PG_HARNESS_MODE=external WORLDSTREAM_PG_HARNESS_ADAPTER_TEST=run \
    WORLDSTREAM_PG_HARNESS_ADMIN_DSN="$admin_dsn" WORLDSTREAM_PG_HARNESS_RUNTIME_DSN="$runtime_dsn" \
    WORLDSTREAM_PG_HARNESS_POOLER_DSN="$pooler_dsn" WORLDSTREAM_PG_HARNESS_POOLER_MODE=transaction \
    WORLDSTREAM_PG_HARNESS_EVIDENCE_FILE="$harness_evidence" WORLDSTREAM_PG_HARNESS_PSQL="$psql_bin" \
    WORLDSTREAM_PG_HARNESS_CARGO="$cargo_bin" scripts/postgres-harness.sh >"$harness_log" 2>&1
  harness_code=$?
  set -e
  if [[ -f "$harness_evidence" ]] && python3 - "$harness_evidence" "$harness_code" <<'PY'
import json, sys
v=json.load(open(sys.argv[1], encoding="utf-8"))
raise SystemExit(0 if v.get("status") == "pass" and int(v.get("exit_code", -1)) == 0 and int(sys.argv[2]) == 0 and not v.get("secrets_emitted") else 1)
PY
  then harness_status="pass"; else harness_status="incomplete"; add_error "redacted_postgres_harness_failed"; fi
else
  harness_status="not_run"
fi

transfer_evidence="$temp_root/transfer.json"
if [[ -n "$sqlite_source" || -z "$sqlite_source" ]]; then
  transfer_args=(--evidence "$transfer_evidence")
  if [[ -n "$sqlite_source" ]]; then
    source_mode="supplied"
    transfer_args+=(--sqlite "$sqlite_source")
  else
    source_mode="built_disposable"
    transfer_args+=(--build-source)
  fi
  set +e
  WORLDSTREAM_PG_TRANSFER_MODE=external WORLDSTREAM_PG_TRANSFER_SQLITE="$sqlite_source" \
    WORLDSTREAM_PG_TRANSFER_ADMIN_DSN="$transfer_admin_dsn" WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN="$transfer_runtime_dsn" \
    WORLDSTREAM_PG_TRANSFER_EVIDENCE_FILE="$transfer_evidence" WORLDSTREAM_PG_TRANSFER_PSQL="$psql_bin" \
    WORLDSTREAM_PG_TRANSFER_CARGO="$cargo_bin" scripts/postgres-transfer-smoke.sh "${transfer_args[@]}" >"$temp_root/transfer.log" 2>&1
  transfer_code=$?
  set -e
  if [[ -f "$transfer_evidence" ]]; then
    transfer_status="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8")).get("status","incomplete"))' "$transfer_evidence")"
    transfer_reason="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8")).get("reason","unknown"))' "$transfer_evidence")"
    transfer_summary_json="$(python3 - "$transfer_evidence" <<'PY'
import json
import sys

evidence = json.load(open(sys.argv[1], encoding="utf-8"))
transfer = evidence.get("transfer", {})
acceptance = evidence.get("acceptance", {})
summary = {
    "scope": transfer.get("scope", "unknown"),
    "mechanics_status": transfer.get("status", "incomplete"),
    "first_chunk": transfer.get("first_chunk", "not_observed"),
    "interruption_first_chunk": transfer.get("interruption_first_chunk", "not_observed"),
    "interruption_abort": transfer.get("interruption_abort", "not_observed"),
    "conflicting_chunk_rejected": transfer.get("conflicting_chunk_rejected", False),
    "checkpoint_resume_replay": transfer.get("checkpoint_resume_replay", "not_observed"),
    "canonical_verification": transfer.get("canonical_verification", "not_observed"),
    "finalization": transfer.get("finalization", "not_observed"),
    "target_authority": transfer.get("target_authority", "not_observed"),
    "target_epoch_fence": transfer.get("target_epoch_fence", "not_observed"),
    "isolated_room_evidence": transfer.get("isolated_room_evidence", "not_observed"),
    "full_room_semantics_hydrated": transfer.get("full_room_semantics_hydrated", False),
    "target_room_readback": transfer.get("target_room_readback", "not_observed"),
    "authority_retry": transfer.get("authority_retry", False),
    "authority_retry_drift_detected": transfer.get("authority_retry_drift_detected", False),
    "whole_deployment_acceptance": transfer.get("whole_deployment_acceptance", acceptance.get("whole_deployment", "incomplete")),
    "blocking_rows": acceptance.get("blocking_rows", ["pack_identities", "resource_identities"]),
}
print(json.dumps(summary, separators=(",", ":")))
PY
)"
  else
    transfer_status="incomplete"; transfer_reason="transfer_evidence_missing"
  fi
  if [[ "$transfer_code" -ne 0 || "$transfer_status" != "pass" ]]; then add_error "sqlite_postgres_transfer_incomplete"; fi
fi

if [[ "$live_adapter_status" == "pass" \
  && "$gateway_status" == "pass" \
  && "$harness_status" == "pass" \
  && "$pooler_status" == "pass" \
  && "$imo50_shared_direct_status" == "pass" \
  && "$imo50_shared_pooler_status" == "pass" \
  && "$transfer_status" == "pass" ]]; then
  overall_status="pass"; overall_reason="disposable_postgresql_shared_conformance_and_transfer_contracts_passed"; exit_code="$EXIT_PASS"
else
  overall_status="incomplete"; overall_reason="one_or_more_live_acceptance_contracts_incomplete"; exit_code="$EXIT_INCOMPLETE"
fi
finish "$exit_code"

#!/usr/bin/env bash
set -euo pipefail

# One-command PostgreSQL evidence runner for IMO-44/IMO-49/IMO-50.
#
# The default mode owns a disposable local cluster.  An external service is
# used only when both WORLDSTREAM_PG_HARNESS_ADMIN_DSN and
# WORLDSTREAM_PG_HARNESS_RUNTIME_DSN are supplied explicitly.  DSNs are kept
# in process memory and are never included in diagnostics or evidence.

readonly HARNESS_SCHEMA="worldstream/postgresql-evidence/v1"
readonly REQUIRED_MAJOR=17
readonly REQUIRED_PATCH=11
readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_WRONG_VERSION=11
readonly EXIT_CONFIGURATION=12
readonly EXIT_VALIDATION=13
readonly EXIT_CLEANUP=14

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

evidence_file="${WORLDSTREAM_PG_HARNESS_EVIDENCE_FILE:-}"
requested_mode="${WORLDSTREAM_PG_HARNESS_MODE:-auto}"
adapter_mode="${WORLDSTREAM_PG_HARNESS_ADAPTER_TEST:-run}"
admin_dsn="${WORLDSTREAM_PG_HARNESS_ADMIN_DSN:-}"
runtime_dsn="${WORLDSTREAM_PG_HARNESS_RUNTIME_DSN:-}"
pooler_dsn="${WORLDSTREAM_PG_HARNESS_POOLER_DSN:-}"
pooler_mode="${WORLDSTREAM_PG_HARNESS_POOLER_MODE:-transaction}"

status="unavailable"
exit_code="$EXIT_UNAVAILABLE"
error_codes=""
postgres_version_num=""
postgres_major=""
postgres_patch=""
version_status="not_checked"
credentials_status="not_checked"
migration_status="not_checked"
migration_contract_status="not_checked"
crash_retry_status="not_checked"
crash_retry_scope="not_checked"
admin_schema_status="not_checked"
direct_status="not_checked"
pooler_status="not_configured"
direct_migration_ledger_status="not_checked"
pooler_migration_ledger_status="not_configured"
adapter_status="not_checked"
adapter_direct_admin_migration_status="not_checked"
adapter_direct_admin_restart_status="not_checked"
adapter_runtime_ddl_status="not_checked"
adapter_direct_runtime_status="not_checked"
adapter_direct_runtime_root_guard_status="not_checked"
adapter_pooler_status="not_configured"
cleanup_status="not_started"
provider_mode=""
owned_cluster=0
owned_temp_root=0
cleanup_done=0
temp_root=""
cluster_dir=""
pg_ctl_bin=""
psql_bin=""
admin_password=""
runtime_password=""
admin_user=""
runtime_user=""
database_name=""
pg_port=""
psql_counter=0
psql_output=""

python_bin="${WORLDSTREAM_PG_HARNESS_PYTHON:-}"
if [[ -z "$python_bin" ]]; then
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif command -v python >/dev/null 2>&1; then
    python_bin="$(command -v python)"
  fi
fi

usage() {
  printf '%s\n' \
    "Usage: scripts/postgres-harness.sh [--evidence PATH]" \
    "" \
    "Default mode owns a temporary local PostgreSQL cluster. For an external" \
    "service, set both WORLDSTREAM_PG_HARNESS_ADMIN_DSN and" \
    "WORLDSTREAM_PG_HARNESS_RUNTIME_DSN. An optional" \
    "WORLDSTREAM_PG_HARNESS_POOLER_DSN exercises transaction-pool mode." \
    "" \
    "The adapter test can be omitted only for boundary tests with" \
    "WORLDSTREAM_PG_HARNESS_ADAPTER_TEST=skip; that result is always" \
    "incomplete and never release evidence."
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --evidence)
      if [[ "$#" -lt 2 ]]; then
        printf '%s\n' 'postgres harness: --evidence requires a path' >&2
        exit "$EXIT_CONFIGURATION"
      fi
      evidence_file="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit "$EXIT_PASS"
      ;;
    *)
      printf '%s\n' 'postgres harness: unknown argument' >&2
      exit "$EXIT_CONFIGURATION"
      ;;
  esac
done

if [[ -z "$python_bin" || ! -x "$python_bin" ]] || ! "$python_bin" -c 'import json, pathlib, secrets, socket' >/dev/null 2>&1; then
  # This is deliberately static: the JSON encoder is unavailable, so do not
  # attempt to interpolate any environment-derived value into the fallback.
  printf '%s\n' '{"adapter_conformance":"not_run","evidence_class":"missing_prerequisite","errors":["python3_unavailable"],"exit_code":10,"reason":"python3_unavailable","release_evidence":false,"schema":"worldstream/postgresql-evidence/v1","secrets_emitted":false,"status":"unavailable"}'
  exit "$EXIT_UNAVAILABLE"
fi

append_error() {
  local code="$1"
  case ";$error_codes;" in
    *";$code;"*) return 0 ;;
  esac
  if [[ -z "$error_codes" ]]; then
    error_codes="$code"
  else
    error_codes="$error_codes;$code"
  fi
}

resolve_tool() {
  local override="$1"
  local name="$2"
  local resolved=""
  if [[ -n "$override" ]]; then
    if [[ -x "$override" ]]; then
      printf '%s' "$override"
    fi
    return 0
  fi
  if resolved="$(command -v "$name" 2>/dev/null)"; then
    printf '%s' "$resolved"
  fi
}

psql_override="${WORLDSTREAM_PG_HARNESS_PSQL:-}"
cargo_override="${WORLDSTREAM_PG_HARNESS_CARGO:-}"
initdb_override="${WORLDSTREAM_PG_HARNESS_INITDB:-}"
pg_ctl_override="${WORLDSTREAM_PG_HARNESS_PG_CTL:-}"

psql_bin="$(resolve_tool "$psql_override" psql)"
cargo_bin="$(resolve_tool "$cargo_override" cargo)"
initdb_bin="$(resolve_tool "$initdb_override" initdb)"
pg_ctl_bin="$(resolve_tool "$pg_ctl_override" pg_ctl)"

cleanup_owned() {
  local stop_status=0
  local server_running=0
  local remove_status=0
  if [[ "$cleanup_done" -eq 1 ]]; then
    return 0
  fi
  cleanup_done=1

  if [[ "$owned_cluster" -eq 1 && -n "$cluster_dir" && -n "$pg_ctl_bin" && -d "$cluster_dir" ]]; then
    if ! "$pg_ctl_bin" -D "$cluster_dir" -m fast -w stop >"$temp_root/cleanup-stop.log" 2>&1; then
      # pg_ctl returns nonzero if the server never started; that is not a
      # cleanup leak, but a live server that refuses to stop is.
      if "$pg_ctl_bin" -D "$cluster_dir" status >"$temp_root/cleanup-status.log" 2>&1; then
        stop_status=1
        server_running=1
      fi
    fi
  fi

  if [[ "$server_running" -eq 0 && -n "$temp_root" && -d "$temp_root" ]]; then
    if ! rm -rf "$temp_root"; then
      remove_status=1
    fi
  fi

  if [[ "$stop_status" -ne 0 || "$remove_status" -ne 0 ]]; then
    cleanup_status="failed"
    return 1
  fi
  if [[ "$owned_temp_root" -eq 1 ]]; then
    cleanup_status="pass"
  elif [[ "$provider_mode" == "external" ]]; then
    cleanup_status="not_owned"
  else
    cleanup_status="not_started"
  fi
  return 0
}

trap cleanup_owned EXIT

write_evidence() {
  local code="$1"
  local error_list="$error_codes"
  local output_path="$evidence_file"
  HARNESS_STATUS="$status" \
    HARNESS_EXIT_CODE="$code" \
    HARNESS_MODE="$provider_mode" \
    HARNESS_VERSION_NUM="$postgres_version_num" \
    HARNESS_MAJOR="$postgres_major" \
    HARNESS_PATCH="$postgres_patch" \
    HARNESS_VERSION_STATUS="$version_status" \
    HARNESS_CREDENTIALS_STATUS="$credentials_status" \
    HARNESS_MIGRATION_STATUS="$migration_status" \
    HARNESS_MIGRATION_CONTRACT_STATUS="$migration_contract_status" \
    HARNESS_CRASH_RETRY_STATUS="$crash_retry_status" \
    HARNESS_CRASH_RETRY_SCOPE="$crash_retry_scope" \
    HARNESS_ADMIN_SCHEMA_STATUS="$admin_schema_status" \
    HARNESS_DIRECT_STATUS="$direct_status" \
    HARNESS_POOLER_STATUS="$pooler_status" \
    HARNESS_DIRECT_MIGRATION_LEDGER_STATUS="$direct_migration_ledger_status" \
    HARNESS_POOLER_MIGRATION_LEDGER_STATUS="$pooler_migration_ledger_status" \
    HARNESS_ADAPTER_STATUS="$adapter_status" \
    HARNESS_ADAPTER_DIRECT_ADMIN_MIGRATION_STATUS="$adapter_direct_admin_migration_status" \
    HARNESS_ADAPTER_DIRECT_ADMIN_RESTART_STATUS="$adapter_direct_admin_restart_status" \
    HARNESS_ADAPTER_RUNTIME_DDL_STATUS="$adapter_runtime_ddl_status" \
    HARNESS_ADAPTER_DIRECT_RUNTIME_STATUS="$adapter_direct_runtime_status" \
    HARNESS_ADAPTER_DIRECT_RUNTIME_ROOT_GUARD_STATUS="$adapter_direct_runtime_root_guard_status" \
    HARNESS_ADAPTER_POOLER_STATUS="$adapter_pooler_status" \
    HARNESS_CLEANUP_STATUS="$cleanup_status" \
    HARNESS_POOLER_CONFIGURED="$([[ -n "$pooler_dsn" ]] && printf true || printf false)" \
    HARNESS_OWNED_CLUSTER="$([[ "$owned_cluster" -eq 1 ]] && printf true || printf false)" \
    HARNESS_ERRORS="$error_list" \
    "$python_bin" - "$output_path" <<'PY'
import json
import os
import sys
from pathlib import Path


def optional_int(name):
    value = os.environ.get(name, "")
    return int(value) if value.isdigit() else None


errors = [item for item in os.environ.get("HARNESS_ERRORS", "").split(";") if item]
code = int(os.environ["HARNESS_EXIT_CODE"])
status = os.environ["HARNESS_STATUS"]
pooler_configured = os.environ.get("HARNESS_POOLER_CONFIGURED") == "true"
owned_cluster = os.environ.get("HARNESS_OWNED_CLUSTER") == "true"
evidence_class = {
    "pass": "live_provider",
    "unavailable": "missing_prerequisite",
    "wrong_version": "unsupported_provider",
    "configuration_error": "configuration_error",
    "incomplete": "incomplete_boundary",
    "validation_failed": "provider_validation_failure",
    "cleanup_failed": "cleanup_failure",
}.get(status, "harness_failure")
evidence = {
    "schema": "worldstream/postgresql-evidence/v1",
    "status": status,
    "release_evidence": code == 0 and status == "pass",
    "evidence_class": evidence_class,
    "exit_code": code,
    "provider_mode": os.environ.get("HARNESS_MODE") or None,
    "postgres": {
        "major": optional_int("HARNESS_MAJOR"),
        "patch": optional_int("HARNESS_PATCH"),
        "server_version_num": os.environ.get("HARNESS_VERSION_NUM") or None,
        "minimum": "17.11",
        "version_status": os.environ.get("HARNESS_VERSION_STATUS"),
    },
    "credentials": {
        "status": os.environ.get("HARNESS_CREDENTIALS_STATUS"),
        "direct_admin_and_runtime_are_separate": os.environ.get("HARNESS_CREDENTIALS_STATUS") == "pass",
    },
    "profiles": {
        "direct_admin": {
            "purpose": "offline_migrations_and_schema_verification",
            "status": os.environ.get("HARNESS_MIGRATION_STATUS"),
        },
        "direct_runtime": {
            "purpose": "read_only_schema_verification_and_runtime_operations",
            "status": os.environ.get("HARNESS_DIRECT_STATUS"),
        },
        "transaction_pooler_runtime": {
            "purpose": "transaction_scoped_runtime_operations_without_session_state",
            "status": os.environ.get("HARNESS_POOLER_STATUS") if pooler_configured else "not_configured",
        },
    },
    "migration": {
        "status": os.environ.get("HARNESS_MIGRATION_STATUS"),
        "path": "direct-admin",
        "forward_only_contract": os.environ.get("HARNESS_MIGRATION_CONTRACT_STATUS"),
    },
    "crash_retry": {
        "status": os.environ.get("HARNESS_CRASH_RETRY_STATUS"),
        "scope": os.environ.get("HARNESS_CRASH_RETRY_SCOPE"),
    },
    "schema_checks": {
        "admin_verification": os.environ.get("HARNESS_ADMIN_SCHEMA_STATUS"),
        "runtime_verification_is_read_only": True,
        "runtime_ddl_denial": os.environ.get("HARNESS_DIRECT_STATUS") == "pass",
        "runtime_migration_ledger_write": os.environ.get(
            "HARNESS_DIRECT_MIGRATION_LEDGER_STATUS"
        ),
    },
    "paths": {
        "direct_runtime": os.environ.get("HARNESS_DIRECT_STATUS"),
        "transaction_pooler": os.environ.get("HARNESS_POOLER_STATUS") if pooler_configured else "not_configured",
        "transaction_pooler_configured": pooler_configured,
        "transaction_pooler_migration_ledger_write": os.environ.get(
            "HARNESS_POOLER_MIGRATION_LEDGER_STATUS"
        ) if pooler_configured else "not_configured",
    },
    "adapter_conformance": os.environ.get("HARNESS_ADAPTER_STATUS"),
    "adapter_evidence": {
        "direct_admin_migrate_verify": os.environ.get("HARNESS_ADAPTER_DIRECT_ADMIN_MIGRATION_STATUS"),
        "direct_admin_restart_idempotent": os.environ.get("HARNESS_ADAPTER_DIRECT_ADMIN_RESTART_STATUS"),
        "runtime_ddl_denied": os.environ.get("HARNESS_ADAPTER_RUNTIME_DDL_STATUS"),
        "direct_runtime_commit_resolution": os.environ.get("HARNESS_ADAPTER_DIRECT_RUNTIME_STATUS"),
        "direct_runtime_root_guard": os.environ.get("HARNESS_ADAPTER_DIRECT_RUNTIME_ROOT_GUARD_STATUS"),
        "transaction_pooler_commit_resolution": os.environ.get("HARNESS_ADAPTER_POOLER_STATUS") if pooler_configured else "not_configured",
    },
    "cleanup": {
        "status": os.environ.get("HARNESS_CLEANUP_STATUS"),
        "owned_temporary_cluster": owned_cluster,
    },
    "errors": errors,
    "secrets_emitted": False,
}
encoded = json.dumps(evidence, sort_keys=True, separators=(",", ":"))
output_path = sys.argv[1] if len(sys.argv) > 1 else ""
if output_path:
    path = Path(output_path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(encoded + "\n", encoding="utf-8")
print(encoded)
PY
}

finish() {
  local requested_code="$1"
  if ! cleanup_owned; then
    append_error "owned_cleanup_failed"
    status="cleanup_failed"
    requested_code="$EXIT_CLEANUP"
  fi
  exit_code="$requested_code"
  write_evidence "$requested_code"
  printf 'postgres harness: status=%s exit_code=%s\n' "$status" "$requested_code" >&2
  exit "$requested_code"
}

fail() {
  status="$1"
  exit_code="$2"
  append_error "$3"
  finish "$exit_code"
}

run_quiet() {
  local log_name="$1"
  shift
  local log_path="$temp_root/$log_name.log"
  if "$@" >"$log_path" 2>&1; then
    return 0
  else
    local command_status="$?"
    return "$command_status"
  fi
}

redacted_log_tail() {
  local log_path="$1"
  [[ -s "$log_path" ]] || return 0
  # Provider output is untrusted: URI encodings, quoted DSNs, and driver
  # formatting make complete shell redaction non-trivial. Keep diagnostics
  # stable and secret-free by withholding the raw log entirely.
  printf '%s\n' 'postgres harness: provider diagnostic output withheld; see stable evidence errors' >&2
}

psql_query() {
  local dsn="$1"
  local password="$2"
  local sql="$3"
  local command_status=0
  psql_counter=$((psql_counter + 1))
  psql_output="$temp_root/psql-$psql_counter.out"
  if [[ -n "$password" ]]; then
    if PGPASSWORD="$password" PGAPPNAME=worldstream-postgres-harness PGCONNECT_TIMEOUT=5 \
      "$psql_bin" "$dsn" --no-psqlrc --quiet --tuples-only --no-align \
      --set=ON_ERROR_STOP=1 --no-password --command "$sql" >"$psql_output" 2>/dev/null; then
      return 0
    else
      command_status="$?"
      return "$command_status"
    fi
  fi
  if (
    unset PGPASSWORD
    PGAPPNAME=worldstream-postgres-harness PGCONNECT_TIMEOUT=5 \
      "$psql_bin" "$dsn" --no-psqlrc --quiet --tuples-only --no-align \
      --set=ON_ERROR_STOP=1 --no-password --command "$sql" >"$psql_output" 2>/dev/null
  ); then
    return 0
  else
    command_status="$?"
    return "$command_status"
  fi
}

first_psql_value() {
  sed -n '1p' "$psql_output" | tr -d '\r' | sed 's/[[:space:]]*$//' | sed 's/^[[:space:]]*//'
}

compact_psql_value() {
  first_psql_value | tr -d '[:space:]'
}

random_token() {
  "$python_bin" -c 'import secrets; print(secrets.token_hex(5))'
}

random_password() {
  "$python_bin" -c 'import secrets, string; alphabet=string.ascii_letters + string.digits; print("".join(secrets.choice(alphabet) for _ in range(32)))'
}

configure_mode() {
  case "$requested_mode" in
    auto)
      if [[ -n "$admin_dsn" || -n "$runtime_dsn" ]]; then
        provider_mode="external"
      else
        provider_mode="managed"
      fi
      ;;
    managed|external)
      provider_mode="$requested_mode"
      ;;
    *)
      fail "configuration_error" "$EXIT_CONFIGURATION" "invalid_mode"
      ;;
  esac

  case "$adapter_mode" in
    run|skip) ;;
    *) fail "configuration_error" "$EXIT_CONFIGURATION" "invalid_adapter_mode" ;;
  esac
  if [[ "$pooler_mode" != "transaction" ]]; then
    fail "configuration_error" "$EXIT_CONFIGURATION" "pooler_mode_must_be_transaction"
  fi
  if [[ "$provider_mode" == "external" ]]; then
    if [[ -z "$admin_dsn" || -z "$runtime_dsn" ]]; then
      fail "configuration_error" "$EXIT_CONFIGURATION" "admin_and_runtime_dsns_required"
    fi
    if [[ "$admin_dsn" == "$runtime_dsn" ]]; then
      fail "configuration_error" "$EXIT_CONFIGURATION" "admin_and_runtime_dsns_must_be_distinct"
    fi
    if [[ -n "$pooler_dsn" && "$pooler_dsn" == "$runtime_dsn" ]]; then
      fail "configuration_error" "$EXIT_CONFIGURATION" "pooler_dsn_must_be_distinct"
    fi
    if [[ -n "$pooler_dsn" && "$pooler_dsn" == "$admin_dsn" ]]; then
      fail "configuration_error" "$EXIT_CONFIGURATION" "pooler_dsn_must_be_distinct_from_admin"
    fi
  elif [[ -n "$admin_dsn" || -n "$runtime_dsn" ]]; then
    fail "configuration_error" "$EXIT_CONFIGURATION" "dsns_not_allowed_in_managed_mode"
  fi
}

check_prerequisites() {
  if [[ -z "$psql_bin" || ! -x "$psql_bin" ]]; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "psql_unavailable"
  fi
  if [[ "$provider_mode" == "managed" ]]; then
    if [[ -z "$initdb_bin" || ! -x "$initdb_bin" ]]; then
      fail "unavailable" "$EXIT_UNAVAILABLE" "initdb_unavailable"
    fi
    if [[ -z "$pg_ctl_bin" || ! -x "$pg_ctl_bin" ]]; then
      fail "unavailable" "$EXIT_UNAVAILABLE" "pg_ctl_unavailable"
    fi
  fi
  if [[ "$adapter_mode" == "run" && ( -z "$cargo_bin" || ! -x "$cargo_bin" ) ]]; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "cargo_unavailable"
  fi
}

setup_managed_cluster() {
  umask 077
  if ! temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-postgres-harness.XXXXXX")"; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "temporary_directory_unavailable"
  fi
  owned_temp_root=1
  owned_cluster=1
  cluster_dir="$temp_root/cluster"
  admin_user="worldstream_admin_$(random_token)"
  runtime_user="worldstream_runtime_$(random_token)"
  database_name="worldstream_harness_$(random_token)"
  admin_password="$(random_password)"
  runtime_password="$(random_password)"
  printf '%s' "$admin_password" >"$temp_root/initdb-password"
  chmod 600 "$temp_root/initdb-password"
  if ! run_quiet initdb \
    "$initdb_bin" -D "$cluster_dir" --username="$admin_user" \
    --pwfile="$temp_root/initdb-password" --auth=scram-sha-256 \
    --no-locale --encoding=UTF8; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "cluster_init_failed"
  fi
  pg_port="$($python_bin -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
  if [[ ! "$pg_port" =~ ^[0-9]+$ ]]; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "port_allocation_failed"
  fi
  if ! run_quiet pg_ctl_start "$pg_ctl_bin" -D "$cluster_dir" -o "-h 127.0.0.1 -p $pg_port" -w start; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "cluster_start_failed"
  fi

  local root_dsn="host=127.0.0.1 port=$pg_port dbname=postgres user=$admin_user"
  if ! psql_query "$root_dsn" "$admin_password" \
    "CREATE ROLE \"$runtime_user\" LOGIN PASSWORD '$runtime_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT"; then
    fail "validation_failed" "$EXIT_VALIDATION" "runtime_role_creation_failed"
  fi
  if ! psql_query "$root_dsn" "$admin_password" \
    "CREATE DATABASE \"$database_name\" OWNER \"$admin_user\""; then
    fail "validation_failed" "$EXIT_VALIDATION" "database_creation_failed"
  fi
  admin_dsn="host=127.0.0.1 port=$pg_port dbname=$database_name user=$admin_user password=$admin_password"
  runtime_dsn="host=127.0.0.1 port=$pg_port dbname=$database_name user=$runtime_user password=$runtime_password"
  if ! psql_query "$admin_dsn" "$admin_password" \
    "REVOKE CREATE ON SCHEMA public FROM PUBLIC"; then
    fail "validation_failed" "$EXIT_VALIDATION" "runtime_schema_create_revoke_failed"
  fi
  if ! psql_query "$admin_dsn" "$admin_password" \
    "GRANT CONNECT ON DATABASE \"$database_name\" TO \"$runtime_user\""; then
    fail "validation_failed" "$EXIT_VALIDATION" "runtime_connect_grant_failed"
  fi
  if ! psql_query "$admin_dsn" "$admin_password" \
    "ALTER DEFAULT PRIVILEGES FOR ROLE \"$admin_user\" IN SCHEMA public GRANT SELECT, INSERT, UPDATE ON TABLES TO \"$runtime_user\""; then
    fail "validation_failed" "$EXIT_VALIDATION" "runtime_table_default_privileges_failed"
  fi
  if ! psql_query "$admin_dsn" "$admin_password" \
    "ALTER DEFAULT PRIVILEGES FOR ROLE \"$admin_user\" IN SCHEMA public GRANT USAGE, SELECT ON SEQUENCES TO \"$runtime_user\""; then
    fail "validation_failed" "$EXIT_VALIDATION" "runtime_sequence_default_privileges_failed"
  fi
}

setup_external_workspace() {
  umask 077
  if ! temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-postgres-harness.XXXXXX")"; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "temporary_directory_unavailable"
  fi
  owned_temp_root=1
}

query_external_version() {
  if ! psql_query "$admin_dsn" "" "SHOW server_version_num"; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "admin_connection_failed"
  fi
  postgres_version_num="$(compact_psql_value)"
  if [[ ! "$postgres_version_num" =~ ^[0-9]+$ ]]; then
    postgres_version_num=""
    fail "validation_failed" "$EXIT_VALIDATION" "invalid_server_version_num"
  fi
  postgres_major=$((postgres_version_num / 10000))
  postgres_patch=$((postgres_version_num % 10000))
  if [[ "$postgres_major" -ne "$REQUIRED_MAJOR" ]]; then
    version_status="wrong_major"
    status="wrong_version"
    exit_code="$EXIT_WRONG_VERSION"
    append_error "unsupported_postgresql_major"
    finish "$EXIT_WRONG_VERSION"
  fi
  if [[ "$postgres_patch" -lt "$REQUIRED_PATCH" ]]; then
    version_status="below_minimum_patch"
    status="wrong_version"
    exit_code="$EXIT_WRONG_VERSION"
    append_error "postgresql_patch_below_17_11"
    finish "$EXIT_WRONG_VERSION"
  fi
  version_status="pass"
}

verify_credentials() {
  local admin_identity=""
  local runtime_identity=""
  if ! psql_query "$admin_dsn" "$admin_password" "SELECT current_user"; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "admin_identity_probe_failed"
  fi
  admin_identity="$(first_psql_value)"
  if [[ -z "$admin_identity" ]]; then
    fail "validation_failed" "$EXIT_VALIDATION" "admin_identity_empty"
  fi
  if ! psql_query "$runtime_dsn" "$runtime_password" "SELECT current_user"; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "runtime_identity_probe_failed"
  fi
  runtime_identity="$(first_psql_value)"
  if [[ -z "$runtime_identity" ]]; then
    fail "validation_failed" "$EXIT_VALIDATION" "runtime_identity_empty"
  fi
  runtime_user="$runtime_identity"
  if [[ "$admin_identity" == "$runtime_identity" ]]; then
    credentials_status="same_role"
    fail "configuration_error" "$EXIT_CONFIGURATION" "admin_and_runtime_roles_not_separate"
  fi
  if ! psql_query "$runtime_dsn" "$runtime_password" \
    "SELECT rolsuper::text || '|' || rolcreaterole::text || '|' || rolcreatedb::text FROM pg_roles WHERE rolname = current_user"; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "runtime_privilege_probe_failed"
  fi
  local privileges="$(compact_psql_value)"
  if [[ "$privileges" != "false|false|false" ]]; then
    credentials_status="runtime_privileged"
    fail "validation_failed" "$EXIT_VALIDATION" "runtime_role_is_not_least_privileged"
  fi
  credentials_status="pass"
}

run_adapter_conformance() {
  if [[ "$adapter_mode" == "skip" ]]; then
    adapter_status="skipped"
    adapter_direct_admin_migration_status="not_run"
    adapter_direct_admin_restart_status="not_run"
    adapter_runtime_ddl_status="not_run"
    adapter_direct_runtime_status="not_run"
    adapter_direct_runtime_root_guard_status="not_run"
    if [[ -z "$pooler_dsn" ]]; then
      adapter_pooler_status="not_configured"
    else
      adapter_pooler_status="not_run"
    fi
    migration_status="skipped"
    append_error "adapter_conformance_not_run"
    return 0
  fi
  local adapter_log="$temp_root/adapter-conformance.log"
  local adapter_command_failed=0
  if [[ -n "$pooler_dsn" ]]; then
    if ! WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN="$admin_dsn" \
      WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN="$runtime_dsn" \
      WORLDSTREAM_POSTGRES_TEST_POOLER_DSN="$pooler_dsn" \
      "$cargo_bin" test --locked -p worldstream-postgres \
      --features conformance-tracer --test postgres_commit \
      live_direct_runtime_and_optional_pooler_conformance -- --nocapture \
      >"$adapter_log" 2>&1; then
      adapter_command_failed=1
    fi
  elif ! (
    unset WORLDSTREAM_POSTGRES_TEST_POOLER_DSN
    WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN="$admin_dsn" \
      WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN="$runtime_dsn" \
      "$cargo_bin" test --locked -p worldstream-postgres \
      --features conformance-tracer --test postgres_commit \
      live_direct_runtime_and_optional_pooler_conformance -- --nocapture
  ) >"$adapter_log" 2>&1; then
    adapter_command_failed=1
  fi
  local marker_failed=0
  if grep -Fq 'LIVE_POSTGRES=PASS direct_admin=migrate+verify major=17' "$adapter_log"; then
    adapter_direct_admin_migration_status="pass"
  else
    adapter_direct_admin_migration_status="failed"
    append_error "direct_admin_migration_evidence_missing"
    marker_failed=1
  fi
  if grep -Fq 'LIVE_POSTGRES=PASS direct_admin=restart-idempotent' "$adapter_log"; then
    adapter_direct_admin_restart_status="pass"
  else
    adapter_direct_admin_restart_status="failed"
    append_error "direct_admin_restart_evidence_missing"
    marker_failed=1
  fi
  if grep -Fq 'LIVE_POSTGRES=PASS runtime_ddl=create_denied' "$adapter_log"; then
    adapter_runtime_ddl_status="pass"
  else
    adapter_runtime_ddl_status="failed"
    append_error "adapter_runtime_ddl_evidence_missing"
    marker_failed=1
  fi
  if grep -Fq 'LIVE_POSTGRES=PASS runtime=direct create+duplicate+conflict+resolve' "$adapter_log"; then
    adapter_direct_runtime_status="pass"
  else
    adapter_direct_runtime_status="failed"
    append_error "direct_runtime_conformance_evidence_missing"
    marker_failed=1
  fi
  if grep -Fq 'LIVE_POSTGRES=PASS runtime=direct same-room-create=reprepare' "$adapter_log"; then
    adapter_direct_runtime_root_guard_status="pass"
  else
    adapter_direct_runtime_root_guard_status="failed"
    append_error "direct_runtime_root_guard_evidence_missing"
    marker_failed=1
  fi
  if [[ -n "$pooler_dsn" ]]; then
    if grep -Fq 'LIVE_POSTGRES_POOLER=PASS path=transaction_pool duplicate+resolve' "$adapter_log"; then
      adapter_pooler_status="pass"
    else
      adapter_pooler_status="failed"
      append_error "pooler_conformance_evidence_missing"
      marker_failed=1
    fi
  else
    adapter_pooler_status="not_configured"
  fi
  if [[ "$adapter_command_failed" -ne 0 ]]; then
    append_error "adapter_conformance_failed"
  fi
  if [[ "$marker_failed" -ne 0 || "$adapter_command_failed" -ne 0 ]]; then
    adapter_status="failed"
    migration_status="failed"
    redacted_log_tail "$adapter_log"
    return 1
  fi
  adapter_status="pass"
  migration_status="pass"
  return 0
}

run_provider_neutral_contracts() {
  if [[ "$adapter_mode" == "skip" ]]; then
    migration_contract_status="not_run"
    crash_retry_status="not_run"
    crash_retry_scope="not_run"
    append_error "provider_neutral_contracts_not_run"
    return 0
  fi

  local contract_log="$temp_root/provider-neutral-contracts.log"
  # These are deliberately the existing provider-neutral fixture tests. They
  # prove forward-prefix validation and the distinction between a rolled-back
  # transaction (safe to retry) and an unknown post-commit result (resolve
  # first). The output is kept private and never promoted to provider evidence.
  if ! (
    unset WORLDSTREAM_POSTGRES_TEST_ADMIN_DSN \
      WORLDSTREAM_POSTGRES_TEST_RUNTIME_DSN \
      WORLDSTREAM_POSTGRES_TEST_POOLER_DSN
    "$cargo_bin" test --locked -p worldstream-postgres --test postgres_commit \
      -- --nocapture
  ) >"$contract_log" 2>&1; then
    migration_contract_status="failed"
    crash_retry_status="failed"
    crash_retry_scope="provider_neutral_fixture"
    append_error "provider_neutral_contracts_failed"
    return 1
  fi

  migration_contract_status="pass"
  crash_retry_status="pass"
  crash_retry_scope="provider_neutral_fixture"
  return 0
}

harden_managed_runtime_ledger() {
  if [[ "$adapter_mode" == "skip" ]]; then
    return 0
  fi
  if ! psql_query "$admin_dsn" "$admin_password" \
    "REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON worldstream_schema_migrations FROM \"$runtime_user\""; then
    fail "validation_failed" "$EXIT_VALIDATION" "managed_runtime_migration_ledger_revoke_failed"
  fi
}

verify_admin_schema() {
  if ! psql_query "$admin_dsn" "$admin_password" \
    "SELECT count(*)::text FROM worldstream_schema_migrations"; then
    admin_schema_status="failed"
    migration_contract_status="failed"
    fail "validation_failed" "$EXIT_VALIDATION" "admin_schema_probe_failed"
  fi
  if [[ "$(compact_psql_value)" != "10" ]]; then
    admin_schema_status="failed"
    migration_contract_status="failed"
    fail "validation_failed" "$EXIT_VALIDATION" "migration_history_incomplete"
  fi

  if ! psql_query "$admin_dsn" "$admin_password" \
    "SELECT CASE WHEN count(*) = 10 AND min(version) = 1 AND max(version) = 10 AND count(DISTINCT version) = 10 AND count(DISTINCT migration_id) = 10 AND count(*) FILTER (WHERE (version, migration_id) IN ((1, '0001-initial-storage-schema'), (2, '0002-operational-authority-v1'), (3, '0003-kernel-conformance-v1'), (4, '0004-kernel-parity-witnesses-v1'), (5, '0005-transfer-publication-v1'), (6, '0006-transfer-target-fence-v1'), (7, '0007-deployment-metadata-v1'), (8, '0008-deployment-identities-v1'), (9, '0009-authority-facts-v1'), (10, '0010-transfer-recovery-completeness-v1'))) = 10 AND count(*) FILTER (WHERE octet_length(checksum) = 32) = 10 AND count(*) FILTER (WHERE logical_history_id = 'worldstream-storage-v1') = 10 AND count(*) FILTER (WHERE octet_length(schema_contract_fingerprint) = 32) = 10 THEN 'pass' ELSE 'fail' END FROM worldstream_schema_migrations"; then
    admin_schema_status="failed"
    migration_contract_status="failed"
    fail "validation_failed" "$EXIT_VALIDATION" "migration_history_shape_probe_failed"
  fi
  if [[ "$(compact_psql_value)" != "pass" ]]; then
    admin_schema_status="failed"
    migration_contract_status="failed"
    fail "validation_failed" "$EXIT_VALIDATION" "migration_history_not_forward_complete"
  fi
  admin_schema_status="pass"
}

verify_runtime_path() {
  local path_name="$1"
  local dsn="$2"
  local password="$3"
  local probe_table="worldstream_harness_runtime_ddl_probe"
  local path_status=""
  local migration_ledger_status=""

  if ! psql_query "$dsn" "$password" "SELECT 1"; then
    path_status="unavailable"
  elif ! psql_query "$dsn" "$password" \
    "SELECT count(*)::text FROM information_schema.tables WHERE table_schema = 'public' AND table_name IN ('worldstream_schema_migrations','worldstream_operation_guards','worldstream_room_roots','worldstream_genesis','worldstream_materializations','worldstream_members','worldstream_timers','worldstream_transitions','worldstream_frames','worldstream_observation_consequences','worldstream_activation_decisions','worldstream_activation_intents','worldstream_activation_operation_receipts','worldstream_room_snapshots','worldstream_semantic_receipts','worldstream_integrity_incidents','worldstream_authority_fences','worldstream_transfer_imports','worldstream_transfer_chunks','worldstream_transfer_target_fence','worldstream_deployment_metadata')"; then
    path_status="unavailable"
  elif [[ "$(compact_psql_value)" != "21" ]]; then
    path_status="schema_mismatch"
  elif ! psql_query "$dsn" "$password" \
    "SELECT (has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'INSERT') OR has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'UPDATE') OR has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'DELETE') OR has_table_privilege(current_user, 'public.worldstream_schema_migrations', 'TRUNCATE'))::text"; then
    path_status="unavailable"
    migration_ledger_status="unavailable"
  elif [[ "$(compact_psql_value)" != "false" ]]; then
    path_status="migration_ledger_write_allowed"
    migration_ledger_status="write_allowed"
  else
    migration_ledger_status="pass"
    if ! psql_query "$dsn" "$password" \
      "SELECT has_schema_privilege(current_user, 'public', 'CREATE')::text"; then
      path_status="unavailable"
    elif [[ "$(compact_psql_value)" != "false" ]]; then
      path_status="ddl_allowed"
    elif psql_query "$dsn" "$password" \
      "CREATE TABLE $probe_table (probe integer NOT NULL)"; then
      # A successful probe is a hard failure. Clean up through admin so even a
      # misconfigured role cannot leave the owned cluster dirty.
      psql_query "$admin_dsn" "$admin_password" "DROP TABLE IF EXISTS $probe_table" >/dev/null 2>&1 || true
      path_status="ddl_allowed"
    else
      path_status="pass"
    fi
  fi

  if [[ "$path_name" == "direct" ]]; then
    direct_status="$path_status"
    direct_migration_ledger_status="$migration_ledger_status"
  else
    pooler_status="$path_status"
    pooler_migration_ledger_status="$migration_ledger_status"
  fi
  if [[ "$path_status" == "unavailable" ]]; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "${path_name}_runtime_probe_unavailable"
  fi
  if [[ "$path_status" != "pass" ]]; then
    if [[ "$path_status" == "migration_ledger_write_allowed" ]]; then
      fail "validation_failed" "$EXIT_VALIDATION" "${path_name}_migration_ledger_write_allowed"
    fi
    fail "validation_failed" "$EXIT_VALIDATION" "${path_name}_runtime_probe_failed"
  fi
}

configure_mode
check_prerequisites

if [[ "$provider_mode" == "managed" ]]; then
  setup_managed_cluster
else
  setup_external_workspace
fi

query_external_version
verify_credentials

if ! run_provider_neutral_contracts; then
  fail "validation_failed" "$EXIT_VALIDATION" "provider_neutral_contracts_failed"
fi

if ! run_adapter_conformance; then
  fail "validation_failed" "$EXIT_VALIDATION" "adapter_conformance_failed"
fi
harden_managed_runtime_ledger
if [[ "$adapter_mode" == "skip" ]]; then
  # The boundary-test mode intentionally cannot claim migration evidence.
  admin_schema_status="not_run"
else
  verify_admin_schema
fi
verify_runtime_path direct "$runtime_dsn" "$runtime_password"

if [[ -n "$pooler_dsn" ]]; then
  if ! psql_query "$pooler_dsn" "" "SELECT current_user"; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "pooler_identity_probe_failed"
  fi
  pooler_identity="$(first_psql_value)"
  if ! psql_query "$runtime_dsn" "$runtime_password" "SELECT current_user"; then
    fail "unavailable" "$EXIT_UNAVAILABLE" "runtime_identity_reprobe_failed"
  fi
  runtime_identity="$(first_psql_value)"
  if [[ -z "$pooler_identity" || "$pooler_identity" != "$runtime_identity" ]]; then
    fail "validation_failed" "$EXIT_VALIDATION" "pooler_runtime_role_mismatch"
  fi
  verify_runtime_path pooler "$pooler_dsn" ""
else
  pooler_status="not_configured"
  append_error "transaction_pooler_not_configured"
fi

if [[ "$adapter_mode" == "skip" || "$pooler_status" == "not_configured" ]]; then
  status="incomplete"
  finish "$EXIT_CONFIGURATION"
fi

status="pass"
finish "$EXIT_PASS"

#!/usr/bin/env bash
set -euo pipefail

required_environment=(
  GITHUB_WORKSPACE
  RUNNER_TEMP
  WORLDSTREAM_BUILD_REVISION
  WORLDSTREAM_PACKAGED_CTL
  WORLDSTREAM_POSTGRES_PASSWORD_FILE
  WORLDSTREAM_PG_DUMP
  WORLDSTREAM_PG_RESTORE
  WORLDSTREAM_PSQL
)
for name in "${required_environment[@]}"; do
  if [[ -z "${!name:-}" ]]; then
    printf '%s\n' "hosted Linux native restore requires $name" >&2
    exit 1
  fi
done

cd "$GITHUB_WORKSPACE"

readonly source_database='worldstream_native_source'
readonly target_database='worldstream_native_restore'
readonly abort_database='worldstream_native_abort'
readonly admin_user='postgres'
readonly runtime_role='worldstream_native_linux_runtime'
readonly target_marker='worldstream/native-postgres-disposable-target/v1'
readonly release_python="$(uv python find 3.14.7)"
readonly report_path='reports/native-linux-postgres-restore.json'
readonly fixture_path='reports/native-linux-transfer-seed.json'
readonly secret_root="$RUNNER_TEMP/worldstream-native-restore-secrets"
readonly passfile="$secret_root/operator.pgpass"
readonly transfer_admin_dsn_file="$secret_root/transfer-admin.dsn"
readonly transfer_runtime_dsn_file="$secret_root/transfer-runtime.dsn"
readonly transfer_abort_admin_dsn_file="$secret_root/transfer-abort-admin.dsn"
readonly work_parent="$RUNNER_TEMP/worldstream-native-restore-work"

for executable in \
  "$WORLDSTREAM_PACKAGED_CTL" \
  "$WORLDSTREAM_PG_DUMP" \
  "$WORLDSTREAM_PG_RESTORE" \
  "$WORLDSTREAM_PSQL"; do
  test -x "$executable"
done
test -f "$WORLDSTREAM_POSTGRES_PASSWORD_FILE"
test ! -L "$WORLDSTREAM_POSTGRES_PASSWORD_FILE"
test "$(stat -c '%a' "$WORLDSTREAM_POSTGRES_PASSWORD_FILE")" = '600'
[[ "$WORLDSTREAM_BUILD_REVISION" =~ ^[0-9a-f]{40}$ ]]
test ! -e "$report_path"
test ! -e "$fixture_path"

IFS= read -r admin_password < "$WORLDSTREAM_POSTGRES_PASSWORD_FILE"
[[ "$admin_password" =~ ^[0-9a-f]{48}$ ]]
runtime_password="$(openssl rand -hex 24)"

clear_secrets() {
  unset PGPASSWORD
  unset WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE
  unset WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE
  unset WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE
  unset WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE
  unset WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION
  admin_password=''
  runtime_password=''
}

provider_system_identifier=''
source_database_oid=''
target_database_oid=''
abort_database_oid=''
runtime_role_oid=''
operator_passfile_fd=-1
operator_passfile_storage_id=''
operator_passfile_file_id=''
operator_passfile_size=''
operator_passfile_sha256=''
transfer_admin_dsn_fd=-1
transfer_runtime_dsn_fd=-1
transfer_abort_admin_dsn_fd=-1
work_parent_storage_id=''
work_parent_file_id=''
preserve_recovery=0
provider_cleanup_complete=0
passfile_scrub_complete=0

scrub_transfer_dsn_files() {
  local descriptor
  for descriptor in "$transfer_admin_dsn_fd" "$transfer_runtime_dsn_fd" "$transfer_abort_admin_dsn_fd"; do
    if [[ "$descriptor" -ge 0 ]]; then
      "$release_python" -I - "$descriptor" <<'PY'
import os
import stat
import sys

descriptor = int(sys.argv[1])
metadata = os.fstat(descriptor)
if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
    raise SystemExit("transfer DSN cleanup authority changed")
os.ftruncate(descriptor, 0)
os.fsync(descriptor)
if os.fstat(descriptor).st_size != 0:
    raise SystemExit("transfer DSN exact scrub failed")
PY
    fi
  done
}

export PGPASSWORD="$admin_password"
psql_admin=(
  "$WORLDSTREAM_PSQL"
  --no-password
  --no-psqlrc
  --host 127.0.0.1
  --port 5432
  --username "$admin_user"
  --quiet
  --set ON_ERROR_STOP=1
)

cleanup_provider_fixtures() {
  if [[ "$provider_cleanup_complete" = '1' || -z "$provider_system_identifier" ]]; then
    return 0
  fi

  local source_guard="NOT EXISTS (SELECT 1 FROM pg_database WHERE datname='$source_database')"
  local target_guard="NOT EXISTS (SELECT 1 FROM pg_database WHERE datname='$target_database')"
  local abort_guard="NOT EXISTS (SELECT 1 FROM pg_database WHERE datname='$abort_database')"
  local role_guard="NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='$runtime_role')"
  local source_drop=''
  local target_drop=''
  local abort_drop=''
  local role_drop=''
  if [[ -n "$source_database_oid" ]]; then
    source_guard="($source_guard OR EXISTS (SELECT 1 FROM pg_database WHERE datname='$source_database' AND oid=$source_database_oid))"
    source_drop="DROP DATABASE IF EXISTS \"$source_database\" WITH (FORCE);"
  fi
  if [[ -n "$target_database_oid" ]]; then
    target_guard="($target_guard OR EXISTS (SELECT 1 FROM pg_database WHERE datname='$target_database' AND oid=$target_database_oid))"
    target_drop="DROP DATABASE IF EXISTS \"$target_database\" WITH (FORCE);"
  fi
  if [[ -n "$abort_database_oid" ]]; then
    abort_guard="($abort_guard OR EXISTS (SELECT 1 FROM pg_database WHERE datname='$abort_database' AND oid=$abort_database_oid))"
    abort_drop="DROP DATABASE IF EXISTS \"$abort_database\" WITH (FORCE);"
  fi
  if [[ -n "$runtime_role_oid" ]]; then
    role_guard="($role_guard OR EXISTS (SELECT 1 FROM pg_roles WHERE rolname='$runtime_role' AND oid=$runtime_role_oid))"
    role_drop="DROP ROLE IF EXISTS \"$runtime_role\";"
  fi

  local cleanup_observation
  export PGPASSWORD="$admin_password"
  if ! cleanup_observation="$("${psql_admin[@]}" --dbname postgres --tuples-only --no-align --field-separator=$'\t' --file=- <<SQL
\set ON_ERROR_STOP on
SELECT (
  control.system_identifier::text = '$provider_system_identifier'
  AND $source_guard
  AND $target_guard
  AND $abort_guard
  AND $role_guard
  AND NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname LIKE 'worldstream_restore_%')
) AS admitted
FROM pg_control_system() AS control
\gset
\if :admitted
$source_drop
$target_drop
$abort_drop
$role_drop
\else
\quit 41
\endif
SELECT control.system_identifier::text,
  (SELECT count(*) FROM pg_database WHERE datname='$source_database'),
  (SELECT count(*) FROM pg_database WHERE datname='$target_database'),
  (SELECT count(*) FROM pg_database WHERE datname='$abort_database'),
  (SELECT count(*) FROM pg_roles WHERE rolname='$runtime_role'),
  (SELECT count(*) FROM pg_roles WHERE rolname LIKE 'worldstream_restore_%')
FROM pg_control_system() AS control;
SQL
)"; then
    unset PGPASSWORD
    return 1
  fi
  unset PGPASSWORD
  if [[ "$cleanup_observation" != "$provider_system_identifier"$'\t0\t0\t0\t0\t0' ]]; then
    return 1
  fi
  provider_cleanup_complete=1
}

scrub_operator_passfile() {
  if [[ "$passfile_scrub_complete" = '1' || "$operator_passfile_fd" -lt 0 ]]; then
    return 0
  fi
  "$release_python" -I - "$operator_passfile_fd" <<'PY'
import os
import stat
import sys

descriptor = int(sys.argv[1])
admitted = os.fstat(descriptor)
if (
    not stat.S_ISREG(admitted.st_mode)
    or admitted.st_nlink != 1
    or admitted.st_uid != os.geteuid()
    or stat.S_IMODE(admitted.st_mode) & 0o077
    or admitted.st_size > 64 * 1024
):
    raise SystemExit("operator passfile cleanup authority changed")
os.ftruncate(descriptor, 0)
os.fsync(descriptor)
after = os.fstat(descriptor)
if (after.st_dev, after.st_ino, after.st_size) != (
    admitted.st_dev,
    admitted.st_ino,
    0,
):
    raise SystemExit("operator passfile exact scrub failed")
PY
  passfile_scrub_complete=1
}

cleanup_on_exit() {
  local exit_status=$?
  local cleanup_status=0
  trap - EXIT
  set +e
  unset WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE
  unset WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE
  unset WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE
  unset WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE
  unset WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION
  if [[ "$preserve_recovery" != '1' ]]; then
    cleanup_provider_fixtures || cleanup_status=1
    scrub_operator_passfile || cleanup_status=1
  fi
  scrub_transfer_dsn_files || cleanup_status=1
  for descriptor in "$transfer_admin_dsn_fd" "$transfer_runtime_dsn_fd" "$transfer_abort_admin_dsn_fd"; do
    if [[ "$descriptor" -ge 0 ]]; then
      exec {descriptor}>&-
    fi
  done
  if [[ "$operator_passfile_fd" -ge 0 ]]; then
    exec {operator_passfile_fd}>&-
  fi
  clear_secrets
  if [[ "$exit_status" -eq 0 && "$cleanup_status" -ne 0 ]]; then
    exit_status=1
  fi
  exit "$exit_status"
}
trap cleanup_on_exit EXIT

provider_system_identifier="$("${psql_admin[@]}" --dbname postgres --tuples-only --no-align --command \
  "SELECT system_identifier::text FROM pg_control_system()")"
[[ "$provider_system_identifier" =~ ^[1-9][0-9]*$ ]]
"${psql_admin[@]}" --dbname postgres --command "CREATE DATABASE $source_database"
source_database_oid="$("${psql_admin[@]}" --dbname postgres --tuples-only --no-align --command \
  "SELECT oid::text FROM pg_database WHERE datname='$source_database'")"
[[ "$source_database_oid" =~ ^[1-9][0-9]*$ ]]
"${psql_admin[@]}" --dbname postgres --command "CREATE DATABASE $target_database"
target_database_oid="$("${psql_admin[@]}" --dbname postgres --tuples-only --no-align --command \
  "SELECT oid::text FROM pg_database WHERE datname='$target_database'")"
[[ "$target_database_oid" =~ ^[1-9][0-9]*$ ]]
"${psql_admin[@]}" --dbname postgres --command "CREATE DATABASE $abort_database"
abort_database_oid="$("${psql_admin[@]}" --dbname postgres --tuples-only --no-align --command \
  "SELECT oid::text FROM pg_database WHERE datname='$abort_database'")"
[[ "$abort_database_oid" =~ ^[1-9][0-9]*$ ]]
"${psql_admin[@]}" --dbname postgres --command \
  "CREATE ROLE $runtime_role LOGIN PASSWORD '$runtime_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION"
runtime_role_oid="$("${psql_admin[@]}" --dbname postgres --tuples-only --no-align --command \
  "SELECT oid::text FROM pg_roles WHERE rolname='$runtime_role'")"
[[ "$runtime_role_oid" =~ ^[1-9][0-9]*$ ]]
"${psql_admin[@]}" --dbname "$source_database" --command \
  "ALTER DEFAULT PRIVILEGES FOR ROLE $admin_user IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO $runtime_role; ALTER DEFAULT PRIVILEGES FOR ROLE $admin_user IN SCHEMA public GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO $runtime_role; GRANT USAGE ON SCHEMA public TO $runtime_role"
"${psql_admin[@]}" --dbname postgres --command \
  "COMMENT ON DATABASE $target_database IS '$target_marker'; ALTER DATABASE $target_database CONNECTION LIMIT 0"
unset PGPASSWORD

test ! -e "$secret_root"
test ! -e "$work_parent"

umask 077
mkdir -m 700 "$secret_root"
set -o noclobber
exec {transfer_admin_dsn_fd}> "$transfer_admin_dsn_file"
exec {transfer_runtime_dsn_fd}> "$transfer_runtime_dsn_file"
exec {transfer_abort_admin_dsn_fd}> "$transfer_abort_admin_dsn_file"
set +o noclobber
printf '%s' "host=127.0.0.1 port=5432 user=$admin_user password=$admin_password dbname=$source_database" >&$transfer_admin_dsn_fd
printf '%s' "host=127.0.0.1 port=5432 user=$runtime_role password=$runtime_password dbname=$source_database" >&$transfer_runtime_dsn_fd
printf '%s' "host=127.0.0.1 port=5432 user=$admin_user password=$admin_password dbname=$abort_database" >&$transfer_abort_admin_dsn_fd

export WORLDSTREAM_PG_TRANSFER_MODE='external'
export WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE="$transfer_admin_dsn_file"
export WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE="$transfer_runtime_dsn_file"
export WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE="$transfer_abort_admin_dsn_file"
export WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE="$runtime_role"
export WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION="$WORLDSTREAM_BUILD_REVISION"
scripts/postgres-transfer-smoke.sh --build-source --evidence "$fixture_path"
unset WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE
unset WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE
unset WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE
unset WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE
unset WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION
scrub_transfer_dsn_files
exec {transfer_admin_dsn_fd}>&-
exec {transfer_runtime_dsn_fd}>&-
exec {transfer_abort_admin_dsn_fd}>&-
transfer_admin_dsn_fd=-1
transfer_runtime_dsn_fd=-1
transfer_abort_admin_dsn_fd=-1

version="$($release_python -I -c 'import tomllib; print(tomllib.load(open("compatibility.toml", "rb"))["contracts"]["product"])')"
archive="dist/linux/worldstream-${version}-linux-x86_64.tar.gz"
test -f "$archive"

mkdir -m 700 "$work_parent"
set -o noclobber
exec {operator_passfile_fd}> "$passfile"
set +o noclobber
operator_passfile_identity="$({
  printf '127.0.0.1:5432:*:%s:%s\n' "$admin_user" "$admin_password"
} | "$release_python" -I -c '
import hashlib
import os
import sys

descriptor = int(sys.argv[1])
value = sys.stdin.buffer.read(64 * 1024 + 1)
if not 0 < len(value) <= 64 * 1024:
    raise SystemExit("operator passfile byte length is invalid")
offset = 0
while offset < len(value):
    written = os.write(descriptor, value[offset:])
    if written <= 0:
        raise SystemExit("operator passfile write made no progress")
    offset += written
os.fsync(descriptor)
metadata = os.fstat(descriptor)
print(
    f"{metadata.st_dev:016x}",
    f"{metadata.st_ino:032x}",
    metadata.st_size,
    "sha256:" + hashlib.sha256(value).hexdigest(),
    sep="\t",
)
' "$operator_passfile_fd")"
IFS=$'\t' read -r operator_passfile_storage_id operator_passfile_file_id \
  operator_passfile_size operator_passfile_sha256 <<<"$operator_passfile_identity"
"$release_python" -I - "$operator_passfile_fd" <<'PY'
import os
import stat
import sys

descriptor = int(sys.argv[1])
os.fsync(descriptor)
admitted = os.fstat(descriptor)
if (
    not stat.S_ISREG(admitted.st_mode)
    or admitted.st_nlink != 1
    or admitted.st_uid != os.geteuid()
    or stat.S_IMODE(admitted.st_mode) != 0o600
    or not 0 < admitted.st_size <= 64 * 1024
):
    raise SystemExit("operator passfile creation was not exact and owner-only")
PY
IFS=$'\t' read -r work_parent_storage_id work_parent_file_id < <(
  "$release_python" -I - "$work_parent" <<'PY'
import os
import stat
import sys

metadata = os.stat(sys.argv[1], follow_symlinks=False)
if not stat.S_ISDIR(metadata.st_mode):
    raise SystemExit("hosted work parent is not an exact directory")
print(f"{metadata.st_dev:016x}", f"{metadata.st_ino:032x}", sep="\t")
PY
)
[[ "$operator_passfile_storage_id" =~ ^[0-9a-f]{16}$ ]]
[[ "$operator_passfile_file_id" =~ ^[0-9a-f]{32}$ ]]
[[ "$operator_passfile_size" =~ ^[1-9][0-9]*$ ]]
[[ "$operator_passfile_sha256" =~ ^sha256:[0-9a-f]{64}$ ]]
[[ "$work_parent_storage_id" =~ ^[0-9a-f]{16}$ ]]
[[ "$work_parent_file_id" =~ ^[0-9a-f]{32}$ ]]

preserve_recovery=1
set +e
"$release_python" -I scripts/postgres-native-restore-hosted-report.py \
  --source native-linux \
  --package-report reports/native-linux-package.json \
  --runtime-report reports/native-linux-package-runtime.json \
  --artifact "$archive" \
  --packaged-control "$WORLDSTREAM_PACKAGED_CTL" \
  --fixture-report "$fixture_path" \
  --source-host 127.0.0.1 \
  --source-port 5432 \
  --source-database "$source_database" \
  --source-username "$admin_user" \
  --source-tls-mode disable \
  --target-host 127.0.0.1 \
  --target-port 5432 \
  --target-database "$target_database" \
  --target-username "$admin_user" \
  --target-tls-mode disable \
  --passfile "$passfile" \
  --passfile-storage-id "$operator_passfile_storage_id" \
  --passfile-file-id "$operator_passfile_file_id" \
  --passfile-size "$operator_passfile_size" \
  --passfile-sha256 "$operator_passfile_sha256" \
  --scrub-passfile-on-success \
  --pg-dump "$WORLDSTREAM_PG_DUMP" \
  --pg-restore "$WORLDSTREAM_PG_RESTORE" \
  --psql "$WORLDSTREAM_PSQL" \
  --work-parent "$work_parent" \
  --work-parent-storage-id "$work_parent_storage_id" \
  --work-parent-file-id "$work_parent_file_id" \
  --timeout-seconds 300 \
  --output "$report_path"
supervisor_status=$?
set -e
if [[ "$supervisor_status" -eq 0 ]]; then
  preserve_recovery=0
elif [[ "$supervisor_status" -eq 43 ]]; then
  preserve_recovery=0
  exit 43
else
  exit "$supervisor_status"
fi

"$release_python" -I - "$report_path" <<'PY'
import json
import sys
from pathlib import Path

report = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
if not (
    report.get("schema") == "worldstream/hosted-native-postgres-restore-evidence/v3"
    and report.get("status") == "pass"
    and report.get("release_evidence") is False
    and report.get("secrets_emitted") is False
    and report.get("native_restore", {}).get("native_witness_minted") is True
    and report.get("cleanup", {}).get("status") == "pass"
    and report.get("cleanup", {}).get("target_database_after_drop") == "absent"
    and report.get("cleanup", {}).get("operator_passfile_disposition")
    == "exact_retained_file_scrubbed_to_zero_length"
):
    raise SystemExit("Linux hosted native restore report failed closed")
PY

cleanup_provider_fixtures

"$release_python" -I - "$secret_root" "$passfile" "$work_parent" "$report_path" \
  "$transfer_admin_dsn_file" "$transfer_runtime_dsn_file" \
  "$transfer_abort_admin_dsn_file" <<'PY'
import json
import os
import stat
import sys
from pathlib import Path

secret_root, passfile, work_parent, report_path, *transfer_dsn_files = map(
    Path, sys.argv[1:]
)
report = json.loads(report_path.read_text(encoding="utf-8"))
expected_count = report["cleanup"]["private_artifact_placeholder_count"]
binding = report.get("private_binding", {})
for path, label in ((secret_root, "secret root"), (work_parent, "work parent")):
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or path.is_symlink():
        raise SystemExit(f"Linux hosted native restore {label} changed")
passfile_metadata = passfile.lstat()
if (
    not stat.S_ISREG(passfile_metadata.st_mode)
    or passfile.is_symlink()
    or passfile_metadata.st_size != 0
):
    raise SystemExit("Linux hosted native restore passfile was not scrubbed")
if sorted(secret_root.iterdir()) != sorted([passfile, *transfer_dsn_files]):
    raise SystemExit("Linux hosted native restore secret inventory changed")
for transfer_dsn_file in transfer_dsn_files:
    metadata = transfer_dsn_file.lstat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or transfer_dsn_file.is_symlink()
        or metadata.st_size != 0
    ):
        raise SystemExit("Linux hosted transfer DSN was not scrubbed")
roots = list(work_parent.iterdir())
binding_root = work_parent / "worldstream-native-platform-binding"
scrubbed_roots = [item for item in roots if item != binding_root]
if (
    len(roots) != 2
    or len(scrubbed_roots) != 1
    or not binding_root.is_dir()
    or binding_root.is_symlink()
    or not scrubbed_roots[0].is_dir()
    or scrubbed_roots[0].is_symlink()
    or binding.get("root_name") != binding_root.name
):
    raise SystemExit("Linux hosted native restore work root changed")
artifacts = list(scrubbed_roots[0].iterdir())
if len(artifacts) != expected_count or any(
    not stat.S_ISREG(item.lstat().st_mode)
    or item.is_symlink()
    or item.stat().st_size != 0
    for item in artifacts
):
    raise SystemExit("Linux hosted native restore private artifacts were not scrubbed")
binding_files = binding.get("files", {})
private_artifacts = list(binding_root.iterdir())
if (
    set(binding_files)
    != {
        "artifact_directory_stdout",
        "snapshot_rebuild_stdout",
        "native_restore_stdout",
        "native_report",
        "native_dump",
    }
    or {item.name for item in private_artifacts}
    != {record.get("name") for record in binding_files.values()}
    or any(
        not stat.S_ISREG(item.lstat().st_mode)
        or item.is_symlink()
        or item.stat().st_size <= 0
        for item in private_artifacts
    )
):
    raise SystemExit("Linux hosted native restore private binding is incomplete")
PY

scrub_operator_passfile
exec {operator_passfile_fd}>&-
operator_passfile_fd=-1
clear_secrets
trap - EXIT
printf '%s\n' 'Linux hosted PostgreSQL 17.11 native backup/restore/recovery passed.'

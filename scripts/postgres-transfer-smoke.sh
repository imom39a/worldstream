#!/usr/bin/env bash
set -euo pipefail

# Bounded live SQLite -> PostgreSQL transfer evidence.
#
# This runner deliberately keeps the Rust integration driver ephemeral.  The
# driver is compiled outside the checkout and uses the reviewed native SQLite,
# PostgreSQL admin/runtime, and transfer-destination APIs directly.  It
# refuses to mutate the target until bounded canonical source evidence has
# passed the native verifier, then stages, resumes, verifies, and finalizes
# under the target epoch fence.  Its JSON is operational evidence only: no
# manifest or release state is changed, and release evidence remains false.

readonly SCHEMA="worldstream/sqlite-postgresql-transfer-evidence/v1"
readonly REQUIRED_MAJOR=17
readonly REQUIRED_PATCH=11
readonly POSTGRES_IMAGE="postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly POSTGRES_REPOSITORY_DIGEST="postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_WRONG_VERSION=11
readonly EXIT_CONFIGURATION=12
readonly EXIT_INCOMPLETE=13
readonly EXIT_PROVIDER=14
readonly EXIT_CLEANUP=14

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

evidence_file="${WORLDSTREAM_PG_TRANSFER_EVIDENCE_FILE:-}"
source_file="${WORLDSTREAM_PG_TRANSFER_SQLITE:-}"
build_source=0
requested_mode="${WORLDSTREAM_PG_TRANSFER_MODE:-docker}"
admin_dsn_file="${WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE:-}"
runtime_dsn_file="${WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE:-}"
abort_admin_dsn_file="${WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE:-}"
runtime_role="${WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE:-}"
docker_bin="${WORLDSTREAM_PG_TRANSFER_DOCKER:-}"
cargo_bin="${WORLDSTREAM_PG_TRANSFER_CARGO:-}"
python_bin="${WORLDSTREAM_PG_TRANSFER_PYTHON:-}"
helper_timeout="${WORLDSTREAM_PG_TRANSFER_TIMEOUT_SECONDS:-180}"
source_revision="${WORLDSTREAM_PG_TRANSFER_SOURCE_REVISION:-}"

status="unavailable"
reason="not_started"
provider_mode=""
owned_container=""
container_started=0
cleanup_status="not_started"
helper_json=""
temp_root=""

usage() {
  printf '%s\n' \
    "Usage: scripts/postgres-transfer-smoke.sh (--sqlite PATH | --build-source) [--evidence PATH]" \
    "" \
    "Default mode creates a digest-pinned disposable postgres:17.11-alpine target with" \
    "separate admin/runtime credentials. Set" \
    "WORLDSTREAM_PG_TRANSFER_MODE=external together with" \
    "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE, WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE, and" \
    "WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE to use bounded owner-only files for" \
    "explicitly supplied," \
    "already-isolated success and abort-probe databases. The abort database" \
    "must be distinct because a provider abort permanently tombstones it." \
    "" \
    "A live result is intentionally incomplete until source and target" \
    "adapters prove exact Room semantics; staged bytes alone are not" \
    "promoted to target authority or release evidence."
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --sqlite)
      if [[ "$#" -lt 2 ]]; then
        printf '%s\n' 'postgres transfer smoke: --sqlite requires a path' >&2
        exit "$EXIT_CONFIGURATION"
      fi
      source_file="$2"
      shift 2
      ;;
    --build-source)
      build_source=1
      shift
      ;;
    --evidence)
      if [[ "$#" -lt 2 ]]; then
        printf '%s\n' 'postgres transfer smoke: --evidence requires a path' >&2
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
      printf '%s\n' 'postgres transfer smoke: unknown argument' >&2
      exit "$EXIT_CONFIGURATION"
      ;;
  esac
done

resolve_python() {
  local override="$1"
  local candidate=""
  local name=""
  if [[ -n "$override" ]]; then
    if [[ -x "$override" ]] \
      && "$override" -c 'import sys; raise SystemExit(sys.version_info[:3] != (3, 14, 7))' \
        >/dev/null 2>&1; then
      printf '%s' "$override"
    fi
    return 0
  fi
  for name in python3 python; do
    candidate="$(command -v "$name" 2>/dev/null || true)"
    if [[ -n "$candidate" && -x "$candidate" ]] \
      && "$candidate" -c 'import sys; raise SystemExit(sys.version_info[:3] != (3, 14, 7))' \
        >/dev/null 2>&1; then
      printf '%s' "$candidate"
      return 0
    fi
  done
  if command -v uv >/dev/null 2>&1; then
    candidate="$(uv run --python 3.14.7 --no-project python -c \
      'import sys; print(sys.executable)' 2>/dev/null || true)"
    if [[ -n "$candidate" && -x "$candidate" ]] \
      && "$candidate" -c 'import sys; raise SystemExit(sys.version_info[:3] != (3, 14, 7))' \
        >/dev/null 2>&1; then
      printf '%s' "$candidate"
    fi
  fi
}

python_bin="$(resolve_python "$python_bin")"

if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then
  printf '%s\n' '{"schema":"worldstream/sqlite-postgresql-transfer-evidence/v1","status":"unavailable","release_evidence":false,"reason":"pinned_python_unavailable","secrets_emitted":false}'
  exit "$EXIT_UNAVAILABLE"
fi

if [[ "$build_source" -eq 0 && -z "$source_file" ]]; then
  status="incomplete"
  reason="sqlite_source_not_supplied"
elif [[ "$build_source" -eq 0 && ( -L "$source_file" || ! -f "$source_file" ) ]]; then
  status="incomplete"
  reason="sqlite_source_not_a_regular_file"
fi

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

docker_bin="$(resolve_tool "$docker_bin" docker)"
cargo_bin="$(resolve_tool "$cargo_bin" cargo)"

write_static_evidence() {
  local code="$1"
  local output_path="$evidence_file"
  STATUS="$status" \
    REASON="$reason" \
    MODE="$provider_mode" \
    CLEANUP="$cleanup_status" \
    EXIT_CODE="$code" \
    "$python_bin" - "$output_path" <<'PY'
import json
import os
import sys
from pathlib import Path

code = int(os.environ.get("EXIT_CODE", "13"))
evidence = {
    "schema": "worldstream/sqlite-postgresql-transfer-evidence/v1",
    "status": os.environ.get("STATUS", "incomplete"),
    "release_evidence": False,
    "exit_code": code,
    "provider_mode": os.environ.get("MODE") or None,
    "reason": os.environ.get("REASON", "unknown"),
    "secrets_emitted": False,
    "postgres": {"required": "17.11", "status": "not_checked"},
    "source": {"status": "not_checked"},
    "transfer": {"status": "not_checked", "finalization": "not_attempted"},
    "cleanup": {"status": os.environ.get("CLEANUP", "not_started")},
}

encoded = json.dumps(evidence, sort_keys=True, separators=(",", ":"))
if sys.argv[1]:
    destination = Path(sys.argv[1])
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(encoded + "\n", encoding="utf-8")
print(encoded)
PY
}

if [[ -n "${WORLDSTREAM_PG_TRANSFER_ADMIN_DSN+x}" \
    || -n "${WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN+x}" \
    || -n "${WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN+x}" ]]; then
  status="incomplete"
  reason="plaintext_dsn_environment_rejected"
  provider_mode="$requested_mode"
  EXIT_CODE="$EXIT_CONFIGURATION" write_static_evidence "$EXIT_CONFIGURATION"
  exit "$EXIT_CONFIGURATION"
fi

if [[ ! "$helper_timeout" =~ ^[1-9][0-9]*$ ]]; then
  status="incomplete"
  reason="invalid_helper_timeout"
  EXIT_CODE="$EXIT_CONFIGURATION" write_static_evidence "$EXIT_CONFIGURATION"
  exit "$EXIT_CONFIGURATION"
fi

if [[ -n "$source_revision" && ! "$source_revision" =~ ^[0-9a-f]{40}$ ]]; then
  status="incomplete"
  reason="invalid_source_revision"
  EXIT_CODE="$EXIT_CONFIGURATION" write_static_evidence "$EXIT_CONFIGURATION"
  exit "$EXIT_CONFIGURATION"
fi

if [[ "$status" == "incomplete" ]]; then
  EXIT_CODE="$EXIT_INCOMPLETE" write_static_evidence "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-transfer-smoke.XXXXXX")"
helper_root="$temp_root/driver"
mkdir -p "$helper_root/src"
if [[ "$build_source" -eq 1 ]]; then
  source_file="$temp_root/source.sqlite"
fi
source_abort_backup="$temp_root/source-transfer-abort.sqlite"
source_transfer_backup="$temp_root/source-transfer-point.sqlite"

cleanup() {
  local cleanup_code=0
  if [[ "$container_started" -eq 1 && -n "$owned_container" ]]; then
    if ! "$docker_bin" rm -f "$owned_container" >/dev/null 2>&1; then
      cleanup_code=1
    fi
  fi
  if [[ "$cleanup_code" -eq 0 ]]; then
    cleanup_status="pass"
  else
    cleanup_status="failed"
  fi
  if [[ -n "$temp_root" && -d "$temp_root" ]]; then
    # This path is a unique mktemp child owned by this invocation.
    rm -rf "$temp_root"
  fi
  if [[ "$cleanup_code" -ne 0 && "$status" == "pass" ]]; then
    status="incomplete"
    reason="owned_target_cleanup_failed"
    # A successful transfer must not survive a failed ownership cleanup as
    # asserted evidence. Replace the report with a minimal fail-closed record
    # and override the process result; no target credentials are exposed.
    EXIT_CODE="$EXIT_CLEANUP" write_static_evidence "$EXIT_CLEANUP"
    exit "$EXIT_CLEANUP"
  fi
}
trap cleanup EXIT

if [[ "$requested_mode" == "external" ]]; then
  provider_mode="external"
  if [[ -z "$admin_dsn_file" || -z "$runtime_dsn_file" || -z "$abort_admin_dsn_file" ]]; then
    status="incomplete"
    reason="external_target_credentials_missing"
    EXIT_CODE="$EXIT_CONFIGURATION" write_static_evidence "$EXIT_CONFIGURATION"
    exit "$EXIT_CONFIGURATION"
  fi
  if [[ "$abort_admin_dsn_file" == "$admin_dsn_file" || "$abort_admin_dsn_file" == "$runtime_dsn_file" ]]; then
    status="incomplete"
    reason="external_abort_target_not_distinct"
    EXIT_CODE="$EXIT_CONFIGURATION" write_static_evidence "$EXIT_CONFIGURATION"
    exit "$EXIT_CONFIGURATION"
  fi
elif [[ "$requested_mode" == "docker" || "$requested_mode" == "auto" ]]; then
  provider_mode="docker"
  if [[ -z "$docker_bin" || ! -x "$docker_bin" ]]; then
    status="unavailable"
    reason="docker_unavailable"
    EXIT_CODE="$EXIT_UNAVAILABLE" write_static_evidence "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi

  admin_password="$($python_bin - <<'PY'
import secrets
print(secrets.token_hex(24))
PY
)"
  runtime_password="$($python_bin - <<'PY'
import secrets
print(secrets.token_hex(24))
PY
)"
  owned_container="worldstream-transfer-smoke-$$"

  if ! "$docker_bin" run --detach --name "$owned_container" \
      --env POSTGRES_USER=admin \
      --env "POSTGRES_PASSWORD=$admin_password" \
      --env POSTGRES_DB=worldstream \
      --publish 127.0.0.1::5432 \
      "$POSTGRES_IMAGE" >"$temp_root/docker-id" 2>"$temp_root/docker-run.log"; then
    status="unavailable"
    reason="postgres_17_11_image_unavailable"
    EXIT_CODE="$EXIT_UNAVAILABLE" write_static_evidence "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  container_started=1

  port=""
  for _ in $(seq 1 60); do
    if "$docker_bin" exec "$owned_container" pg_isready -U admin -d worldstream \
        >"$temp_root/pg-ready.log" 2>&1; then
      port="$($docker_bin port "$owned_container" 5432/tcp 2>/dev/null | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p' | head -n 1)"
      if [[ -n "$port" ]]; then
        break
      fi
    fi
    sleep 1
  done
  if [[ -z "$port" ]]; then
    status="unavailable"
    reason="postgres_target_did_not_start"
    EXIT_CODE="$EXIT_UNAVAILABLE" write_static_evidence "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi

  # Fixed role names and generated hexadecimal passwords keep this SQL
  # injection-free; credentials remain process-local and never enter logs.
  setup_sql="CREATE ROLE runtime LOGIN PASSWORD '$runtime_password' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION; ALTER DEFAULT PRIVILEGES FOR ROLE admin IN SCHEMA public GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO runtime; ALTER DEFAULT PRIVILEGES FOR ROLE admin IN SCHEMA public GRANT USAGE, SELECT, UPDATE ON SEQUENCES TO runtime; GRANT USAGE ON SCHEMA public TO runtime;"
  if ! "$docker_bin" exec --env "PGPASSWORD=$admin_password" "$owned_container" \
      psql -v ON_ERROR_STOP=1 -U admin -d worldstream -Atqc "$setup_sql" \
      >"$temp_root/role-setup.log" 2>&1; then
    status="unavailable"
    reason="runtime_role_setup_failed"
    EXIT_CODE="$EXIT_UNAVAILABLE" write_static_evidence "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  if ! "$docker_bin" exec --env "PGPASSWORD=$admin_password" "$owned_container" \
      createdb -U admin worldstream_abort_probe \
      >"$temp_root/abort-database-setup.log" 2>&1; then
    status="unavailable"
    reason="abort_probe_database_setup_failed"
    EXIT_CODE="$EXIT_UNAVAILABLE" write_static_evidence "$EXIT_UNAVAILABLE"
    exit "$EXIT_UNAVAILABLE"
  fi
  admin_dsn_file="$temp_root/admin.dsn"
  runtime_dsn_file="$temp_root/runtime.dsn"
  abort_admin_dsn_file="$temp_root/abort-admin.dsn"
  umask 077
  printf '%s' "host=127.0.0.1 port=$port user=admin password=$admin_password dbname=worldstream" >"$admin_dsn_file"
  printf '%s' "host=127.0.0.1 port=$port user=runtime password=$runtime_password dbname=worldstream" >"$runtime_dsn_file"
  printf '%s' "host=127.0.0.1 port=$port user=admin password=$admin_password dbname=worldstream_abort_probe" >"$abort_admin_dsn_file"
  chmod 600 "$admin_dsn_file" "$runtime_dsn_file" "$abort_admin_dsn_file"
  unset admin_password runtime_password
elif [[ "$requested_mode" != "external" ]]; then
  status="incomplete"
  reason="unsupported_mode"
  EXIT_CODE="$EXIT_CONFIGURATION" write_static_evidence "$EXIT_CONFIGURATION"
  exit "$EXIT_CONFIGURATION"
fi

if [[ -z "$cargo_bin" || ! -x "$cargo_bin" ]]; then
  status="unavailable"
  reason="cargo_unavailable"
  EXIT_CODE="$EXIT_UNAVAILABLE" write_static_evidence "$EXIT_UNAVAILABLE"
  exit "$EXIT_UNAVAILABLE"
fi

helper_manifest="$helper_root/Cargo.toml"
helper_source="$helper_root/src/main.rs"
cat >"$helper_manifest" <<EOF
[package]
name = "worldstream-postgres-transfer-smoke-driver"
version = "0.1.0"
edition = "2024"
rust-version = "1.97.1"
publish = false

[dependencies]
postgres = "=0.19.14"
rusqlite = { git = "https://github.com/rusqlite/rusqlite.git", rev = "229140734a4a60cc9fa34507fe79cb2277142f49", default-features = false, features = ["bundled"] }
serde_json = "=1.0.151"
zeroize = "=1.9.0"
worldstream-core = { path = "$workspace_dir/crates/worldstream-core", features = ["conformance-tracer"] }
worldstream-backup = { path = "$workspace_dir/crates/worldstream-backup" }
worldstream-postgres = { path = "$workspace_dir/crates/worldstream-postgres" }
worldstream-runtime = { path = "$workspace_dir/crates/worldstream-runtime" }
worldstream-sqlite = { path = "$workspace_dir/crates/worldstream-sqlite" }
worldstream-transfer = { path = "$workspace_dir/crates/worldstream-transfer" }
EOF

cat >"$helper_source" <<'RS'
#![recursion_limit = "256"]

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::{Path, PathBuf},
};

use postgres::{Client, NoTls};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use zeroize::Zeroizing;
use worldstream_core::{
    agent_heist_digest, builtin_counter_registry, builtin_worldstream_registry, AccessModeV1,
    CanonicalJsonV1, CanonicalRequestHashV1, CompleteHeadV1, CoreRoomStateV1, CoreTraceV1,
    IntegrityGenerationV1, MembershipStandingV1, MembershipV1, OperationIdentityV1, PackDigestV1,
    PackGenesisRequestV1, PackRevisionLockV1, ParticipantActionRequestV1,
    ParticipantActionV1, PreparedAuthorityWitnessV1, PreparedExistingIntentV1,
    PreparedObservationConsequenceV1, PreparedRoomCommitV1, PreparedTimerMutationKindV1,
    PrincipalKindV1, RecordedStimulusV1, ResolveOutcomeV1,
    StoredSemanticResultV1, TimerFiredRequestV1, TimerFiredV1, TransitionV1,
    CORE_SCHEMA_VERSION,
};
use worldstream_backup::native_sqlite::{
    canonical_activity_materialization_hash, canonical_core_materialization_hash,
    canonical_genesis_hash, assess_restore_readiness, NativeSqliteLimits,
    NativeSqliteRestoreInputV1, extract_operational_rows,
    extract_restore_evidence, verify_file,
};
use worldstream_backup::DigestV1 as BackupDigestV1;
use worldstream_postgres::{
    PostgresAdmin, PostgresConnectionConfig, PostgresConnectionPath, PostgresRoomStore,
    PostgresTransferDestination, postgres_backend_fingerprint,
};
use worldstream_runtime::SecretSource;
use worldstream_sqlite::{
    SqliteCanonicalRecordKindV1, SqliteRoomStore, SqliteSourceTransferStateV1,
};
use worldstream_transfer::{
    BackendFingerprintV1, BundleProfileV1, CanonicalRecordKindV1, DigestV1,
    NativeSqliteTransferAdapterV1, NativeSqliteTransferSpecV1, PackIdentityV1,
    ResourceKindV1, ResourcePayloadV1, SessionStatePolicyV1, TargetFingerprintV1, RecordKindV1,
    TransferBundleV1, TransferChunkV1, TransferDestinationV1, TransferImportSessionV1,
    TransferStateV1,
    abort_whole_deployment, finalize_whole_deployment,
};

const EXIT_INCOMPLETE: i32 = 13;
const EXIT_WRONG_VERSION: i32 = 11;
const EXIT_PROVIDER: i32 = 14;
const MAX_SOURCE_ROOMS: usize = 100_000;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_DSN_BYTES: usize = 64 * 1024;

fn read_dsn_file(variable: &str) -> Result<Zeroizing<String>, ()> {
    let path = env::var_os(variable)
        .filter(|value| !value.is_empty())
        .ok_or(())?;
    let bytes = Zeroizing::new(
        SecretSource::File(PathBuf::from(path))
            .read_bounded(MAX_DSN_BYTES)
            .map_err(|_| ())?,
    );
    let value = Zeroizing::new(
        std::str::from_utf8(bytes.as_slice())
            .map_err(|_| ())?
            .to_owned(),
    );
    if value.trim().is_empty() || value.as_bytes().contains(&0) {
        return Err(());
    }
    Ok(value)
}

struct RoomEvidenceRow {
    room_id: String,
    room_seq: i64,
    pack_digest: String,
    core_hash: String,
    activity_hash: String,
    head_hash: String,
    head_bytes: Vec<u8>,
    pack_revision_lock_bytes: Option<Vec<u8>>,
    genesis_bytes: Option<Vec<u8>>,
    core_bytes: Option<Vec<u8>>,
    activity_bytes: Option<Vec<u8>>,
}

struct SourceCanonicalEvidence {
    canonical_records: Vec<worldstream_transfer::LogicalRecordV1>,
    packs: Vec<PackIdentityV1>,
    deployment_identity: Option<worldstream_transfer::DeploymentIdentityV1>,
    lineage_id: Option<String>,
    source_epoch: Option<u64>,
    resources: Vec<worldstream_transfer::ResourceIdentityV1>,
    resource_payloads: Vec<ResourcePayloadV1>,
    rooms: Vec<Value>,
    canonical_rooms: BTreeMap<String, CanonicalRoomEvidence>,
    transition_count: usize,
    missing: Vec<Value>,
    metadata_missing: Vec<Value>,
    isolated_rooms: BTreeSet<String>,
    stored_bytes: usize,
}

struct CanonicalRoomEvidence {
    pack_revision_lock_bytes: Vec<u8>,
    _genesis_bytes: Vec<u8>,
    _head_bytes: Vec<u8>,
    _core_state_bytes: Vec<u8>,
    _activity_state_bytes: Vec<u8>,
    transition_bytes: Vec<Vec<u8>>,
}

#[derive(Clone)]
struct SeedTimer {
    timer_id: String,
    generation: u64,
    scheduled_for: String,
    payload_bytes: Vec<u8>,
    state: &'static str,
}

#[derive(Clone)]
struct SeedFrame {
    member_id: String,
    frame_seq: u64,
    cause_room_seq: u64,
    payload_hash: String,
    payload_bytes: Vec<u8>,
}

#[derive(Clone)]
struct SeedRoom {
    room_id: String,
    head: CompleteHeadV1,
    head_bytes: Vec<u8>,
    pack_lock_bytes: Vec<u8>,
    genesis_bytes: Vec<u8>,
    core_bytes: Vec<u8>,
    activity_bytes: Vec<u8>,
    transitions: Vec<(String, Vec<u8>)>,
    memberships: Vec<MembershipV1>,
    timers: Vec<SeedTimer>,
    frames: Vec<SeedFrame>,
    receipt: Option<StoredSemanticResultV1>,
    integrity_status: &'static str,
    integrity_generation: u64,
}

fn redacted_subject(value: &str) -> String {
    DigestV1::hash(value.as_bytes()).to_string()
}

fn byte_evidence(bytes: Option<&[u8]>) -> Value {
    match bytes {
        Some(bytes) => json!({
            "status": "observed",
            "bytes": bytes.len(),
            "digest": DigestV1::hash(bytes).to_string()
        }),
        None => json!({"status": "missing"}),
    }
}

fn stored_digest(value: &str) -> Option<BackupDigestV1> {
    BackupDigestV1::parse(value.strip_prefix("blake3:").unwrap_or(value).to_owned()).ok()
}

fn missing_evidence(code: &'static str, scope: &'static str, subject: Option<&str>, detail: &'static str) -> Value {
    let mut value = json!({
        "code": code,
        "scope": scope,
        "detail": detail,
        "action": "persist and verify the source fact before retrying"
    });
    if let Some(subject) = subject {
        value["subject"] = json!(redacted_subject(subject));
    }
    value
}

fn restore_input_label(input: NativeSqliteRestoreInputV1) -> &'static str {
    match input {
        NativeSqliteRestoreInputV1::Backend => "backend",
        NativeSqliteRestoreInputV1::NativePoint => "native_point",
        NativeSqliteRestoreInputV1::StorageEpoch => "storage_epoch",
        NativeSqliteRestoreInputV1::MigrationContract => "migration_contract",
        NativeSqliteRestoreInputV1::PackIdentities => "pack_identities",
        NativeSqliteRestoreInputV1::ResourceIdentities => "resource_identities",
        NativeSqliteRestoreInputV1::RoomMembership => "room_membership",
        NativeSqliteRestoreInputV1::DeploymentLineage => "deployment_lineage",
    }
}

fn missing_restore_input(input: NativeSqliteRestoreInputV1) -> Value {
    let label = restore_input_label(input);
    let mut value = missing_evidence(
        "source_restore_input_missing",
        "deployment",
        None,
        "the source did not persist this exact restore/transfer input",
    );
    value["input"] = json!(label);
    value
}

fn table_exists(connection: &rusqlite::Transaction<'_>, table: &str) -> Result<bool, ()> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1)",
            [table],
            |row| row.get(0),
        )
        .map_err(|_| ())
}

fn extract_source_evidence(
    source: &Path,
    verified_point: &Path,
    source_report: &worldstream_backup::native_sqlite::NativeSqliteVerificationReportV1,
) -> Result<SourceCanonicalEvidence, ()> {
    // Use the reviewed SQLite export seam before opening the independent
    // read-only witness below; this avoids holding a read transaction while
    // the adapter opens its controlled connection.
    let api_export = SqliteRoomStore::open(source)
        .ok()
        .and_then(|store| store.export_canonical_evidence().ok());
    let deployment_identity = api_export
        .as_ref()
        .map(|export| export.deployment_identity().clone());
    let resource_payloads = api_export
        .as_ref()
        .map(|export| export.resource_payloads().to_vec())
        .unwrap_or_default();
    let restore_evidence =
        extract_restore_evidence(verified_point, NativeSqliteLimits::default()).ok();
    let mut metadata_missing = Vec::new();
    if let Some(restore_evidence) = &restore_evidence {
        let readiness = assess_restore_readiness(restore_evidence);
        metadata_missing = readiness
            .missing
            .into_iter()
            .map(missing_restore_input)
            .collect();
    } else {
        metadata_missing.push(missing_evidence(
            "source_restore_evidence_unavailable",
            "deployment",
            None,
            "the bounded native restore evidence could not be read",
        ));
    }
    let isolated_rooms = restore_evidence
        .as_ref()
        .map(|evidence| {
            evidence
                .integrity
                .iter()
                .filter(|(_, (status, _))| {
                    matches!(
                        status.to_ascii_lowercase().as_str(),
                        "faulted" | "quarantined" | "isolated"
                    )
                })
                .map(|(room_id, _)| room_id.clone())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let connection =
        Connection::open_with_flags(verified_point, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| ())?;
    connection.pragma_update(None, "query_only", true).map_err(|_| ())?;
    let transaction = connection.unchecked_transaction().map_err(|_| ())?;
    let has_metadata = table_exists(&transaction, "canonical_export_metadata")?;
    let mut lineage_id = None;
    let mut source_epoch = None;
    let mut missing = Vec::new();

    if has_metadata {
        match transaction
            .query_row(
                "SELECT deployment_lineage, storage_epoch FROM canonical_export_metadata WHERE metadata_id = 1",
                (),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|_| ())?
        {
            Some((lineage, epoch)) if !lineage.is_empty() && epoch > 0 => {
                lineage_id = Some(lineage);
                source_epoch = Some(u64::try_from(epoch).map_err(|_| ())?);
            }
            _ => missing.push(missing_evidence(
                "canonical_export_metadata_invalid",
                "deployment",
                None,
                "canonical export metadata is absent or invalid",
            )),
        }
    } else {
        missing.push(missing_evidence(
            "deployment_lineage_absent",
            "deployment",
            None,
            "the persisted SQLite schema has no deployment-lineage identity",
        ));
        missing.push(missing_evidence(
            "storage_epoch_absent",
            "deployment",
            None,
            "the persisted SQLite schema has no source storage epoch",
        ));
    }

    let has_rooms = table_exists(&transaction, "rooms")?;
    let has_genesis = table_exists(&transaction, "room_genesis")?;
    let has_materializations = table_exists(&transaction, "room_materializations")?;
    if !has_rooms {
        missing.push(missing_evidence(
            "rooms_relation_absent",
            "room",
            None,
            "the source Room root relation is absent",
        ));
    }
    if !has_genesis {
        missing.push(missing_evidence(
            "room_genesis_relation_absent",
            "room",
            None,
            "the source Room Genesis relation is absent",
        ));
    }
    if !has_materializations {
        missing.push(missing_evidence(
            "room_materializations_relation_absent",
            "room",
            None,
            "the source Room materialization relation is absent",
        ));
    }

    let mut rows = Vec::new();
    if has_rooms && has_genesis && has_materializations {
        let mut statement = transaction
            .prepare(
                "SELECT r.room_id, r.room_seq, r.pack_digest, r.core_state_hash, \
                 r.activity_state_hash, r.genesis_or_transition_hash, r.complete_head_bytes, \
                 g.pack_revision_lock_bytes, g.genesis_bytes, m.core_state_bytes, \
                 m.activity_state_bytes \
                 FROM rooms AS r \
                 LEFT JOIN room_genesis AS g ON g.room_id = r.room_id \
                 LEFT JOIN room_materializations AS m ON m.room_id = r.room_id \
                 ORDER BY r.room_id LIMIT ?1",
            )
            .map_err(|_| ())?;
        let queried = statement
            .query_map([i64::try_from(MAX_SOURCE_ROOMS).map_err(|_| ())?], |row| {
                Ok(RoomEvidenceRow {
                    room_id: row.get(0)?,
                    room_seq: row.get(1)?,
                    pack_digest: row.get(2)?,
                    core_hash: row.get(3)?,
                    activity_hash: row.get(4)?,
                    head_hash: row.get(5)?,
                    head_bytes: row.get(6)?,
                    pack_revision_lock_bytes: row.get(7)?,
                    genesis_bytes: row.get(8)?,
                    core_bytes: row.get(9)?,
                    activity_bytes: row.get(10)?,
                })
            })
            .map_err(|_| ())?;
        for row in queried {
            rows.push(row.map_err(|_| ())?);
        }
    }

    let mut seen_rooms = BTreeSet::new();
    let mut canonical_records = Vec::new();
    if let Some(lineage) = &lineage_id {
        canonical_records.push(
            worldstream_transfer::LogicalRecordV1::canonical(
                0,
                CanonicalRecordKindV1::DeploymentLineage,
                "deployment/lineage",
                lineage.as_bytes(),
            )
            .map_err(|_| ())?,
        );
    }
    if let Some(epoch) = source_epoch {
        let epoch_bytes = epoch.to_string();
        canonical_records.push(
            worldstream_transfer::LogicalRecordV1::canonical(
                canonical_records.len() as u64,
                CanonicalRecordKindV1::StorageEpoch,
                "deployment/epoch",
                epoch_bytes.as_bytes(),
            )
            .map_err(|_| ())?,
        );
    }
    let mut rooms = Vec::new();
    let mut canonical_rooms = BTreeMap::new();
    let mut transition_count = 0_usize;
    let mut packs = Vec::<PackIdentityV1>::new();
    let mut stored_bytes = 0usize;
    for row in rows {
        if !seen_rooms.insert(row.room_id.clone()) {
            missing.push(missing_evidence(
                "duplicate_room_evidence",
                "room",
                Some(&row.room_id),
                "the source returned more than one Room root for one identity",
            ));
            continue;
        }
        let subject = redacted_subject(&row.room_id);
        let isolated = isolated_rooms.contains(&row.room_id);
        let raw_entries = [
            ("pack_revision_lock", row.pack_revision_lock_bytes.as_deref()),
            ("genesis", row.genesis_bytes.as_deref()),
            ("head", Some(row.head_bytes.as_slice())),
            ("core", row.core_bytes.as_deref()),
            ("activity", row.activity_bytes.as_deref()),
        ];
        for (_, bytes) in raw_entries {
            if let Some(bytes) = bytes {
                stored_bytes = stored_bytes.checked_add(bytes.len()).ok_or(())?;
            }
        }
        if stored_bytes > MAX_SOURCE_BYTES {
            missing.push(missing_evidence(
                "source_evidence_bound_exceeded",
                "deployment",
                None,
                "the bounded source evidence byte limit was exceeded",
            ));
            break;
        }

        let pack_lock = row
            .pack_revision_lock_bytes
            .as_deref()
            .zip(row.pack_digest.parse::<PackDigestV1>().ok());
        let pack_lock_valid = pack_lock.as_ref().is_some_and(|(bytes, digest)| {
            PackRevisionLockV1::from_canonical_bytes(bytes, digest).is_ok()
        });
        let parsed_pack = pack_lock.and_then(|(bytes, digest)| {
            let lock = PackRevisionLockV1::from_canonical_bytes(bytes, &digest).ok()?;
            let digest = DigestV1::from_bytes(digest.digest().as_bytes()).ok()?;
            PackIdentityV1::new(lock.pack_id, lock.explanatory_version, digest).ok()
        });
        if !pack_lock_valid {
            missing.push(missing_evidence(
                "pack_revision_lock_unverifiable",
                "room",
                Some(&row.room_id),
                "the persisted pack revision lock is absent or does not verify against rooms.pack_digest",
            ));
        } else if let Some(candidate) = parsed_pack {
            if !packs.contains(&candidate) {
                packs.push(candidate);
            }
        }

        let head_valid = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&row.head_bytes)
            .is_ok_and(|head| {
                head.room_id().to_string() == row.room_id
                    && head.room_seq().get() == u64::try_from(row.room_seq).unwrap_or_default()
                    && head.core_schema_version() == CORE_SCHEMA_VERSION
                    && head.pack_digest().to_string() == row.pack_digest
                    && head.core_state_hash().to_string() == row.core_hash
                    && head.activity_state_hash().to_string() == row.activity_hash
                    && source_report
                        .room_lineage_digests
                        .get(&row.room_id)
                        .is_some_and(|digest| stored_digest(&row.head_hash).as_ref() == Some(digest))
            });
        let core_valid = row
            .core_bytes
            .as_deref()
            .is_some_and(|bytes| {
                canonical_core_materialization_hash(bytes, &row.pack_digest).as_ref()
                    == stored_digest(&row.core_hash).as_ref()
            });
        let activity_valid = row
            .activity_bytes
            .as_deref()
            .is_some_and(|bytes| {
                canonical_activity_materialization_hash(
                    bytes,
                    row.pack_digest.strip_prefix("blake3:").unwrap_or(&row.pack_digest),
                )
                .as_ref()
                    == stored_digest(&row.activity_hash).as_ref()
            });
        let genesis_valid = row
            .genesis_bytes
            .as_deref()
            .is_some_and(|bytes| canonical_genesis_hash(bytes).is_some());
        let raw_complete = row.pack_revision_lock_bytes.is_some()
            && row.genesis_bytes.is_some()
            && row.core_bytes.is_some()
            && row.activity_bytes.is_some();
        let room_complete = if isolated {
            raw_complete
        } else {
            pack_lock_valid && head_valid && core_valid && activity_valid && genesis_valid
        };
        if !isolated {
            if row.genesis_bytes.is_none() {
                missing.push(missing_evidence(
                    "room_genesis_bytes_absent",
                    "room",
                    Some(&row.room_id),
                    "the Room Genesis bytes are not persisted",
                ));
            } else if !genesis_valid {
                missing.push(missing_evidence(
                    "room_genesis_bytes_unverifiable",
                    "room",
                    Some(&row.room_id),
                    "the persisted Room Genesis bytes do not verify as typed canonical evidence",
                ));
            }
        }
        if !isolated && !head_valid {
            missing.push(missing_evidence(
                "room_head_bytes_unverifiable",
                "room",
                Some(&row.room_id),
                "the persisted Complete Head bytes do not verify against the Room root and lineage",
            ));
        }
        if !isolated && !core_valid {
            missing.push(missing_evidence(
                "room_core_bytes_unverifiable",
                "room",
                Some(&row.room_id),
                "the persisted Core bytes are absent or do not match rooms.core_state_hash",
            ));
        }
        if !isolated && !activity_valid {
            missing.push(missing_evidence(
                "room_activity_bytes_unverifiable",
                "room",
                Some(&row.room_id),
                "the persisted Activity bytes are absent or do not match rooms.activity_state_hash",
            ));
        }

        let mut room_value = json!({
            "subject": subject,
            "status": if isolated { "isolated" } else if room_complete { "verified" } else { "incomplete" },
            "pack_revision_lock": byte_evidence(row.pack_revision_lock_bytes.as_deref()),
            "genesis": byte_evidence(row.genesis_bytes.as_deref()),
            "head": byte_evidence(Some(row.head_bytes.as_slice())),
            "core": byte_evidence(row.core_bytes.as_deref()),
            "activity": byte_evidence(row.activity_bytes.as_deref()),
            "canonical_records": []
        });
        if room_complete {
            let record_specs = [
                (CanonicalRecordKindV1::RoomGenesis, "genesis", row.genesis_bytes.as_deref()),
                (CanonicalRecordKindV1::RoomHead, "head", Some(row.head_bytes.as_slice())),
                (CanonicalRecordKindV1::CoreMaterialization, "core-materialization", row.core_bytes.as_deref()),
                (CanonicalRecordKindV1::ActivityMaterialization, "activity-materialization", row.activity_bytes.as_deref()),
                (CanonicalRecordKindV1::ArtifactMetadata, "pack-revision-lock", row.pack_revision_lock_bytes.as_deref()),
            ];
            let mut record_summary = Vec::new();
            for (kind, suffix, bytes) in record_specs {
                let Some(bytes) = bytes else { return Err(()); };
                let identity = match kind {
                    CanonicalRecordKindV1::RoomGenesis => format!("room/{}/genesis", row.room_id),
                    CanonicalRecordKindV1::RoomHead => format!("room/{}/head", row.room_id),
                    CanonicalRecordKindV1::CoreMaterialization => format!("room/{}/core", row.room_id),
                    CanonicalRecordKindV1::ActivityMaterialization => format!("room/{}/activity", row.room_id),
                    CanonicalRecordKindV1::ArtifactMetadata => {
                        format!("room/{}/pack-revision-lock", row.room_id)
                    }
                    _ => unreachable!("fixed Room evidence kind"),
                };
                let record = worldstream_transfer::LogicalRecordV1::canonical(
                    canonical_records.len() as u64,
                    kind,
                    &identity,
                    bytes,
                )
                .map_err(|_| ())?;
                record_summary.push(json!({
                    "kind": suffix,
                    "subject": redacted_subject(&identity),
                    "bytes": bytes.len(),
                    "digest": record.digest().to_string()
                }));
                canonical_records.push(record);
            }
            room_value["canonical_records"] = Value::Array(record_summary);
            canonical_rooms.insert(
                row.room_id.clone(),
                CanonicalRoomEvidence {
                    pack_revision_lock_bytes: row.pack_revision_lock_bytes.clone().ok_or(())?,
                    _genesis_bytes: row.genesis_bytes.clone().ok_or(())?,
                    _head_bytes: row.head_bytes.clone(),
                    _core_state_bytes: row.core_bytes.clone().ok_or(())?,
                    _activity_state_bytes: row.activity_bytes.clone().ok_or(())?,
                    transition_bytes: Vec::new(),
                },
            );
        }
        rooms.push(room_value);
    }
    let deployment_record_count = if lineage_id.is_some() && source_epoch.is_some() {
        2
    } else {
        0
    };
    if let Some(ref export) = api_export {
        let exported_isolated = export.isolated_rooms().iter().cloned().collect::<BTreeSet<_>>();
        if exported_isolated != isolated_rooms {
            missing.push(missing_evidence(
                "isolated_room_membership_mismatch",
                "room",
                None,
                "the reviewed SQLite export did not report the same isolated Room set as the direct integrity witness",
            ));
        }
        if export.deployment_lineage() != lineage_id.as_deref().unwrap_or_default()
            || export.storage_epoch() != source_epoch.unwrap_or_default()
        {
            missing.push(missing_evidence(
                "canonical_export_metadata_mismatch",
                "deployment",
                None,
                "the SQLite canonical export API disagrees with the direct metadata witness",
            ));
        }
        if export.deployment_identity().packs().is_empty() {
            missing.push(missing_evidence(
                "deployment_pack_identity_absent",
                "deployment",
                None,
                "the authoritative deployment identity has no retained Pack",
            ));
        } else if export
            .deployment_identity()
            .packs()
            .iter()
            .any(|expected| !packs.contains(expected))
        {
            missing.push(missing_evidence(
                "deployment_pack_identity_mismatch",
                "deployment",
                None,
                "a retained Pack has no exact persisted Room revision lock",
            ));
        } else {
            packs = export.deployment_identity().packs().to_vec();
        }
        let raw_room_records = canonical_records[deployment_record_count..]
            .iter()
            .map(|record| (record.identity(), record.bytes()))
            .collect::<BTreeMap<_, _>>();
        let api_room_records = export
            .records()
            .iter()
            .map(|record| (record.identity(), record.bytes()))
            .collect::<BTreeMap<_, _>>();
        if raw_room_records
            .iter()
            .any(|(identity, bytes)| api_room_records.get(identity).copied() != Some(*bytes))
            || raw_room_records.len().saturating_add(source_report.transition_count)
                != export.records().len()
        {
            missing.push(missing_evidence(
                "canonical_export_byte_mismatch",
                "room",
                None,
                "the direct source witness did not match the reviewed SQLite export seam",
            ));
        } else {
            let mut api_records = canonical_records[..deployment_record_count].to_vec();
            for (offset, record) in export.records().iter().enumerate() {
                let kind = match record.kind() {
                    SqliteCanonicalRecordKindV1::RoomGenesis => {
                        CanonicalRecordKindV1::RoomGenesis
                    }
                    SqliteCanonicalRecordKindV1::RoomTransition => {
                        CanonicalRecordKindV1::RoomTransition
                    }
                    SqliteCanonicalRecordKindV1::RoomHead => CanonicalRecordKindV1::RoomHead,
                    SqliteCanonicalRecordKindV1::CoreMaterialization => {
                        CanonicalRecordKindV1::CoreMaterialization
                    }
                    SqliteCanonicalRecordKindV1::ActivityMaterialization => {
                        CanonicalRecordKindV1::ActivityMaterialization
                    }
                    SqliteCanonicalRecordKindV1::PackRevisionLock => {
                        CanonicalRecordKindV1::ArtifactMetadata
                    }
                };
                api_records.push(
                    worldstream_transfer::LogicalRecordV1::canonical(
                        (deployment_record_count + offset) as u64,
                        kind,
                        record.identity(),
                        record.bytes(),
                    )
                    .map_err(|_| ())?,
                );
            }
            canonical_records = api_records;
        }
    } else {
        missing.push(missing_evidence(
            "sqlite_canonical_export_unavailable",
            "deployment",
            None,
            "the reviewed SQLite canonical export seam could not be opened and exported",
        ));
    }
    if deployment_identity.is_none() {
        missing.push(missing_evidence(
            "deployment_identity_absent",
            "deployment",
            None,
            "the source did not expose the complete pack/resource identity witness",
        ));
    }
    if let Some(restore_evidence) = restore_evidence {
        for (room_id, records) in restore_evidence.canonical_records {
            let Some(room) = canonical_rooms.get_mut(&room_id) else {
                continue;
            };
            for record in records.into_iter().filter(|record| record.room_seq > 0) {
                room.transition_bytes.push(record.bytes);
                transition_count = transition_count.saturating_add(1);
            }
        }
        if transition_count != source_report.transition_count {
            missing.push(missing_evidence(
                "transition_evidence_incomplete",
                "room",
                None,
                "the exact persisted Transition set did not match native verification",
            ));
        }
    } else {
        missing.push(missing_evidence(
            "transition_evidence_unavailable",
            "room",
            None,
            "the exact persisted Transition set could not be extracted",
        ));
    }
    // The legacy native-backup verifier has a separate migration-contract
    // vocabulary. The transfer gate below relies on the newer authenticated
    // SQLite canonical export plus the exact operational-row extractor; keep
    // the independent backup readiness result visible in evidence without
    // treating that unrelated verifier as a source mutation or authority.
    let expected_canonical_count = api_export
        .as_ref()
        .map_or(0, |export| export.records().len())
        .saturating_add(deployment_record_count);
    if canonical_records.is_empty() || canonical_records.len() != expected_canonical_count {
        missing.push(missing_evidence(
            "room_canonical_evidence_incomplete",
            "room",
            None,
            "every persisted Room must contribute verified Genesis, Head, Core, and Activity records",
        ));
    }
    transaction.commit().map_err(|_| ())?;
    Ok(SourceCanonicalEvidence {
        canonical_records,
        packs,
        deployment_identity: deployment_identity.clone(),
        lineage_id,
        source_epoch,
        resources: deployment_identity
            .as_ref()
            .map(|identity| identity.resources().to_vec())
            .unwrap_or_default(),
        resource_payloads,
        rooms,
        canonical_rooms,
        transition_count,
        missing,
        metadata_missing,
        isolated_rooms,
        stored_bytes,
    })
}

fn emit(mut value: Value, code: i32) -> ! {
    if let Some(object) = value.as_object_mut() {
        object.insert("secrets_emitted".to_owned(), Value::Bool(false));
        object.insert("release_evidence".to_owned(), Value::Bool(false));
        object.insert("exit_code".to_owned(), json!(code));
    }
    println!(
        "{}",
        serde_json::to_string(&value).unwrap_or_else(|_| "{\"status\":\"incomplete\"}".to_owned())
    );
    std::process::exit(code);
}

fn base(mode: &str) -> Value {
    json!({
        "schema": "worldstream/sqlite-postgresql-transfer-evidence/v1",
        "status": "incomplete",
        "provider_mode": mode,
        "postgres": {"required": "17.11", "status": "not_checked"},
        "source": {"status": "not_checked"},
        "transfer": {"status": "not_checked", "finalization": "not_attempted"},
        "cleanup": {"status": "not_applicable"}
    })
}

fn incomplete(mode: &str, reason: &str, mut value: Value) -> ! {
    if let Some(object) = value.as_object_mut() {
        object.insert("status".to_owned(), json!("incomplete"));
        object.insert("reason".to_owned(), json!(reason));
        object.insert("provider_mode".to_owned(), json!(mode));
    }
    emit(value, EXIT_INCOMPLETE)
}

fn provider_failure(mode: &str, reason: &str, mut value: Value) -> ! {
    if let Some(object) = value.as_object_mut() {
        object.insert("status".to_owned(), json!("incomplete"));
        object.insert("reason".to_owned(), json!(reason));
        object.insert("provider_mode".to_owned(), json!(mode));
    }
    emit(value, EXIT_PROVIDER)
}

fn wrong_version(mode: &str, version: &str, mut value: Value) -> ! {
    if let Some(object) = value.as_object_mut() {
        object.insert("status".to_owned(), json!("wrong_version"));
        object.insert("reason".to_owned(), json!("postgres_target_below_17_11"));
        object.insert("provider_mode".to_owned(), json!(mode));
        object.insert("postgres".to_owned(), json!({
            "required": "17.11",
            "server_version_num": version,
            "status": "wrong_version"
        }));
    }
    emit(value, EXIT_WRONG_VERSION)
}

fn membership_bytes(membership: &MembershipV1) -> Result<Vec<u8>, ()> {
    CanonicalJsonV1::parse(&serde_json::to_vec(membership).map_err(|_| ())?)
        .and_then(|value| value.to_bytes())
        .map_err(|_| ())
}

fn seed_frames(plan: &PreparedRoomCommitV1) -> Result<Vec<SeedFrame>, ()> {
    let PreparedExistingIntentV1::Advance(advance) = plan.intent() else {
        return Err(());
    };
    advance
        .delivery_consequences
        .iter()
        .map(|consequence| match consequence {
            PreparedObservationConsequenceV1::ObservationFrame(frame) => Ok(SeedFrame {
                member_id: frame.member_id().to_string(),
                frame_seq: frame.frame_seq(),
                cause_room_seq: frame.cause_room_seq().get(),
                payload_hash: frame.payload_hash().to_string(),
                payload_bytes: frame.canonical_payload_bytes().to_vec(),
            }),
            PreparedObservationConsequenceV1::ResetRequired(_)
            | PreparedObservationConsequenceV1::VisibilityLost(_) => Err(()),
        })
        .collect()
}

fn seed_room_from_trace(
    room_id: &str,
    trace: &CoreTraceV1,
    transition_ids: &[&str],
    timers: Vec<SeedTimer>,
    frames: Vec<SeedFrame>,
    receipt: Option<StoredSemanticResultV1>,
) -> Result<SeedRoom, ()> {
    if trace.transitions().len() != transition_ids.len() {
        return Err(());
    }
    Ok(SeedRoom {
        room_id: room_id.to_owned(),
        head: trace.head().clone(),
        head_bytes: trace.head().canonical_bytes().map_err(|_| ())?,
        pack_lock_bytes: trace
            .retained_pack()
            .ok_or(())?
            .revision_lock()
            .canonical_bytes()
            .map_err(|_| ())?,
        genesis_bytes: trace.genesis_bytes().map_err(|_| ())?,
        core_bytes: trace.core_state().canonical_bytes().map_err(|_| ())?,
        activity_bytes: trace.activity_state().to_bytes().map_err(|_| ())?,
        transitions: trace
            .transitions()
            .iter()
            .zip(transition_ids)
            .map(|(transition, id)| {
                transition
                    .canonical_bytes()
                    .map(|bytes| ((*id).to_owned(), bytes))
                    .map_err(|_| ())
            })
            .collect::<Result<Vec<_>, _>>()?,
        memberships: trace.core_state().memberships().values().cloned().collect(),
        timers,
        frames,
        receipt,
        integrity_status: "healthy",
        integrity_generation: 1,
    })
}

fn pack_identity(room: &SeedRoom) -> Result<PackIdentityV1, ()> {
    let lock = PackRevisionLockV1::from_canonical_bytes(
        &room.pack_lock_bytes,
        room.head.pack_digest(),
    )
    .map_err(|_| ())?;
    PackIdentityV1::new(
        lock.pack_id,
        lock.explanatory_version,
        DigestV1::from_bytes(room.head.pack_digest().digest().as_bytes()).map_err(|_| ())?,
    )
    .map_err(|_| ())
}

fn seed_room(connection: &Connection, room: &SeedRoom) -> Result<(), ()> {
    let head = &room.head;
    let room_seq = i64::try_from(head.room_seq().get()).map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO rooms(room_id, room_status, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes) VALUES (?1, 'active', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                room.room_id,
                room_seq,
                head.genesis_or_transition_hash().to_string(),
                head.core_schema_version(),
                head.pack_digest().to_string(),
                head.core_state_hash().to_string(),
                head.activity_state_hash().to_string(),
                head.authoritative_state_hash().to_string(),
                room.head_bytes,
            ],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO room_genesis(room_id, pack_revision_lock_bytes, genesis_bytes) VALUES (?1, ?2, ?3)",
            rusqlite::params![room.room_id, room.pack_lock_bytes, room.genesis_bytes],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO room_materializations(room_id, core_state_bytes, activity_state_bytes) VALUES (?1, ?2, ?3)",
            rusqlite::params![room.room_id, room.core_bytes, room.activity_bytes],
        )
        .map_err(|_| ())?;
    for membership in &room.memberships {
        let principal_kind = match membership.principal_kind() {
            PrincipalKindV1::Human => "human",
            PrincipalKindV1::Agent => "agent",
        };
        let standing = match membership.standing() {
            MembershipStandingV1::Enabled => "enabled",
            MembershipStandingV1::Suspended => "suspended",
            MembershipStandingV1::Departed => "departed",
        };
        let access_mode = match membership.access_mode() {
            AccessModeV1::Participant => "participant",
            AccessModeV1::Spectator => "spectator",
            AccessModeV1::Operator => "operator",
        };
        let frame_head = room
            .frames
            .iter()
            .filter(|frame| frame.member_id == membership.member_id().as_str())
            .map(|frame| frame.frame_seq)
            .max()
            .unwrap_or(0);
        connection
            .execute(
                "INSERT INTO room_members(room_id, member_id, principal_id, principal_kind, standing, access_mode, role, membership_bytes, membership_generation, frame_head) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9)",
                rusqlite::params![
                    room.room_id,
                    membership.member_id().to_string(),
                    membership.principal_id().to_string(),
                    principal_kind,
                    standing,
                    access_mode,
                    membership.role(),
                    membership_bytes(membership)?,
                    i64::try_from(frame_head).map_err(|_| ())?,
                ],
            )
            .map_err(|_| ())?;
    }
    connection
        .execute(
            "INSERT INTO room_integrity(room_id, status, generation) VALUES (?1, ?2, ?3)",
            rusqlite::params![
                room.room_id,
                room.integrity_status,
                i64::try_from(room.integrity_generation).map_err(|_| ())?,
            ],
        )
        .map_err(|_| ())?;
    for (transition_id, transition_bytes) in &room.transitions {
        let transition = TransitionV1::from_canonical_bytes(transition_bytes).map_err(|_| ())?;
        let transition_head = transition.complete_head();
        connection
            .execute(
                "INSERT INTO transitions(room_id, transition_id, room_seq, transition_hash, previous_lineage_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, transition_bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    room.room_id,
                    transition_id,
                    i64::try_from(transition.room_seq().get()).map_err(|_| ())?,
                    transition.transition_hash().to_string(),
                    transition.previous_lineage_hash().to_string(),
                    transition_head.core_schema_version(),
                    transition_head.pack_digest().to_string(),
                    transition_head.core_state_hash().to_string(),
                    transition_head.activity_state_hash().to_string(),
                    transition_head.authoritative_state_hash().to_string(),
                    transition_bytes,
                ],
            )
            .map_err(|_| ())?;
    }
    for frame in &room.frames {
        connection
            .execute(
                "INSERT INTO observation_frames(room_id, member_id, frame_seq, cause_room_seq, payload_hash, payload_bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    room.room_id,
                    frame.member_id,
                    i64::try_from(frame.frame_seq).map_err(|_| ())?,
                    i64::try_from(frame.cause_room_seq).map_err(|_| ())?,
                    frame.payload_hash,
                    frame.payload_bytes,
                ],
            )
            .map_err(|_| ())?;
    }
    for timer in &room.timers {
        connection
            .execute(
                "INSERT INTO timers(room_id, timer_id, generation, scheduled_for, payload_bytes, state) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    room.room_id,
                    timer.timer_id,
                    i64::try_from(timer.generation).map_err(|_| ())?,
                    timer.scheduled_for,
                    timer.payload_bytes,
                    timer.state,
                ],
            )
            .map_err(|_| ())?;
    }
    if let Some(receipt) = &room.receipt {
        let identity_bytes = receipt
            .operation_identity()
            .canonical_bytes()
            .map_err(|_| ())?;
        let semantic_input_bytes = receipt
            .semantic_input()
            .canonical_bytes()
            .map_err(|_| ())?;
        let semantic_time_bytes = receipt.canonical_semantic_time_bytes().map_err(|_| ())?;
        let basis_bytes = receipt.canonical_basis_head_bytes().map_err(|_| ())?;
        let transition_seq = receipt
            .transition_seq()
            .map(|sequence| i64::try_from(sequence.get()).map_err(|_| ()))
            .transpose()?;
        connection
            .execute(
                "INSERT INTO semantic_receipts(room_id, operation_kind, operation_identity_bytes, codec_id, canonical_request_hash, basis_complete_head_bytes, semantic_input_bytes, semantic_time_bytes, resolution_kind, transition_seq, stored_resolution_bytes) VALUES (?1, ?2, ?3, 'worldstream/operation-receipt/v1', ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    room.room_id,
                    receipt.operation_identity().operation_kind(),
                    identity_bytes,
                    receipt.canonical_request_hash().as_bytes().as_slice(),
                    basis_bytes,
                    semantic_input_bytes,
                    semantic_time_bytes,
                    receipt.resolution_kind(),
                    transition_seq,
                    receipt.canonical_receipt_bytes(),
                ],
            )
            .map_err(|_| ())?;
    }
    connection
        .execute(
            "INSERT INTO room_snapshots(room_id, room_seq, snapshot_schema_version, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes, core_state_bytes, activity_state_bytes) VALUES (?1, ?2, 'worldstream/paired-snapshot/v1', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                room.room_id,
                room_seq,
                head.genesis_or_transition_hash().to_string(),
                head.core_schema_version(),
                head.pack_digest().to_string(),
                head.core_state_hash().to_string(),
                head.activity_state_hash().to_string(),
                head.authoritative_state_hash().to_string(),
                room.head_bytes,
                room.core_bytes,
                room.activity_bytes,
            ],
        )
        .map_err(|_| ())?;
    if room.integrity_status != "healthy" {
        connection
            .execute(
                "INSERT INTO integrity_incidents(room_id, incident_seq, generation, status, reason_code, details_bytes) VALUES (?1, 1, ?2, ?3, 'transfer_smoke_preexisting_isolation', ?4)",
                rusqlite::params![
                    room.room_id,
                    i64::try_from(room.integrity_generation).map_err(|_| ())?,
                    room.integrity_status,
                    br#"{"fixture":"isolated_raw_bytes"}"#.as_slice(),
                ],
            )
            .map_err(|_| ())?;
    }
    Ok(())
}

fn seed_global_authority(
    connection: &Connection,
    member_room_id: &str,
    member_id: &str,
    runner_room_id: &str,
    runner_member_id: &str,
) -> Result<(), ()> {
    let principal_id = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    let runner_id = "01ARZ3NDEKTSV4RRFFQ69G5FH0";
    let member_capability = "01ARZ3NDEKTSV4RRFFQ69G5FF0";
    let host_capability = "01ARZ3NDEKTSV4RRFFQ69G5FF1";
    let runner_capability = "01ARZ3NDEKTSV4RRFFQ69G5FF2";
    connection.execute(
        "INSERT INTO principals(principal_id, principal_kind, authority_status, principal_generation) VALUES (?1, 'human', 'enabled', 1)",
        [principal_id],
    ).map_err(|_| ())?;
    connection.execute(
        "INSERT INTO runners(runner_id, owner_principal_id, authority_status, runner_generation) VALUES (?1, ?2, 'enabled', 1)",
        [runner_id, principal_id],
    ).map_err(|_| ())?;
    for (capability_id, token, profile, target_room, target_member, runner) in [
        (member_capability, vec![0x11_u8; 32], "room_member", Some(member_room_id), Some(member_id), None),
        (host_capability, vec![0x22_u8; 32], "host_operator", None, None, None),
        (runner_capability, vec![0x33_u8; 32], "runner_control", None, None, Some(runner_id)),
    ] {
        connection.execute(
            "INSERT INTO capabilities(capability_id, token_hash, principal_id, profile_kind, target_room_id, target_member_id, runner_id, authority_generation) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1)",
            rusqlite::params![capability_id, token, principal_id, profile, target_room, target_member, runner],
        ).map_err(|_| ())?;
    }
    for (capability_id, scope) in [
        (member_capability, "room:act"),
        (host_capability, "operator:room_admin"),
        (runner_capability, "activation:claim"),
    ] {
        connection.execute(
            "INSERT INTO capability_scopes(capability_id, scope) VALUES (?1, ?2)",
            [capability_id, scope],
        ).map_err(|_| ())?;
    }
    connection.execute(
        "INSERT INTO runner_capability_memberships(capability_id, room_id, member_id) VALUES (?1, ?2, ?3)",
        [runner_capability, runner_room_id, runner_member_id],
    ).map_err(|_| ())?;
    for (
        audit_seq,
        change_id,
        actor,
        request_hash_byte,
        result_kind,
        target_kind,
        target_id,
        secondary_target_id,
        change_kind,
        checked_at,
    ) in [
        (
            1_i64,
            "01ARZ3NDEKTSV4RRFFQ69G5FK0",
            None,
            0x40_u8,
            "authority_bootstrapped",
            "bootstrap",
            principal_id,
            Some(host_capability),
            "bootstrap_authority",
            "2026-08-21T12:00:00Z",
        ),
        (
            2,
            "01ARZ3NDEKTSV4RRFFQ69G5FK1",
            Some(principal_id),
            0x41,
            "runner_registered",
            "runner",
            runner_id,
            None,
            "register_runner",
            "2026-08-21T12:00:01Z",
        ),
        (
            3,
            "01ARZ3NDEKTSV4RRFFQ69G5FK2",
            Some(principal_id),
            0x42,
            "capability_registered",
            "capability",
            member_capability,
            None,
            "register_capability",
            "2026-08-21T12:00:02Z",
        ),
        (
            4,
            "01ARZ3NDEKTSV4RRFFQ69G5FK3",
            Some(principal_id),
            0x43,
            "capability_registered",
            "capability",
            runner_capability,
            None,
            "register_capability",
            "2026-08-21T12:00:03Z",
        ),
    ] {
        let request_hash = vec![request_hash_byte; 32];
        connection.execute(
            "INSERT INTO authority_change_receipts(change_id, authenticated_principal, request_hash, result_kind, target_kind, target_id, secondary_target_id, resulting_generation, checked_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8)",
            rusqlite::params![
                change_id,
                actor,
                request_hash,
                result_kind,
                target_kind,
                target_id,
                secondary_target_id,
                checked_at,
            ],
        ).map_err(|_| ())?;
        connection.execute(
            "INSERT INTO authority_audit(audit_seq, change_id, actor_principal_id, target_kind, target_id, secondary_target_id, change_kind, prior_generation, resulting_generation, checked_at, reason_code, request_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 1, ?8, NULL, ?9)",
            rusqlite::params![
                audit_seq,
                change_id,
                actor,
                target_kind,
                target_id,
                secondary_target_id,
                change_kind,
                checked_at,
                request_hash,
            ],
        ).map_err(|_| ())?;
    }
    Ok(())
}

fn build_disposable_source(path: &Path) -> Result<(), ()> {
    let counter_room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let counter_member_id = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
    let counter_principal_id = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    let counter_registry = builtin_counter_registry().map_err(|_| ())?;
    let counter_membership = MembershipV1::new(
        counter_member_id.parse().map_err(|_| ())?,
        counter_principal_id.parse().map_err(|_| ())?,
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".to_owned()),
    )
    .map_err(|_| ())?;
    let counter_request = PackGenesisRequestV1 {
        room_id: counter_room_id.parse().map_err(|_| ())?,
        pack_digest: worldstream_core::counter_v2_digest(),
        configuration: CanonicalJsonV1::parse(br#"{"initial_value":0,"maximum_value":4}"#)
            .map_err(|_| ())?,
        room_seed: "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
            .parse()
            .map_err(|_| ())?,
        created_at: "2026-08-21T12:00:00Z".parse().map_err(|_| ())?,
        initial_core_state: CoreRoomStateV1::active([counter_membership]).map_err(|_| ())?,
    };
    let mut counter_trace = CoreTraceV1::create_from_retained_for_conformance(
        counter_registry
            .prepare_genesis_for_new_room(&counter_request)
            .map_err(|_| ())?,
    )
    .map_err(|_| ())?;
    let action_definition = counter_trace
        .retained_pack()
        .ok_or(())?
        .descriptor()
        .actions
        .iter()
        .find(|action| action.action_type == "increment")
        .ok_or(())?;
    let counter_member: worldstream_core::MemberId =
        counter_member_id.parse().map_err(|_| ())?;
    let counter_action_id: worldstream_core::ActionId =
        "01ARZ3NDEKTSV4RRFFQ69G5FE0".parse().map_err(|_| ())?;
    let counter_payload = CanonicalJsonV1::parse(br"{}").map_err(|_| ())?;
    let counter_action_request = ParticipantActionRequestV1::new(
        counter_room_id.parse().map_err(|_| ())?,
        counter_member.clone(),
        counter_action_id.clone(),
        counter_trace.head().room_seq(),
        "increment",
        counter_payload.clone(),
    );
    let counter_stimulus = RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
            member_id: counter_member,
            action_id: counter_action_id,
            action_type: "increment".to_owned(),
            payload_schema_digest: action_definition.payload_schema.schema_digest.clone(),
            canonical_payload: counter_payload,
            exact_basis_head: counter_trace.head().clone(),
            admitted_at: "2026-08-21T12:00:01Z".parse().map_err(|_| ())?,
        });
    let counter_prepared = counter_trace
        .prepare(counter_stimulus.clone())
        .map_err(|_| ())?;
    let counter_plan = PreparedRoomCommitV1::for_action_for_conformance(
        &counter_trace,
        &counter_action_request,
        counter_prepared,
        "01ARZ3NDEKTSV4RRFFQ69G5FE0".parse().map_err(|_| ())?,
        IntegrityGenerationV1::new(1).map_err(|_| ())?,
        PreparedAuthorityWitnessV1::mint_for_conformance(
            "transfer-smoke-counter-action",
            counter_principal_id.parse().map_err(|_| ())?,
            1,
            &CanonicalJsonV1::parse(br#"{"revoked":false,"scope":"action"}"#)
                .map_err(|_| ())?,
        )
        .map_err(|_| ())?,
        &BTreeMap::from([(counter_member_id.parse().map_err(|_| ())?, 0_u64)]),
    )
    .map_err(|_| ())?;
    let counter_frames = seed_frames(&counter_plan)?;
    let counter_receipt = counter_plan.semantic_result().clone();
    counter_trace
        .advance_for_conformance(counter_stimulus)
        .map_err(|_| ())?;
    let counter_room = seed_room_from_trace(
        counter_room_id,
        &counter_trace,
        &["01ARZ3NDEKTSV4RRFFQ69G5FE0"],
        Vec::new(),
        counter_frames,
        Some(counter_receipt),
    )?;

    let heist_room_id = "01ARZ3NDEKTSV4RRFFQ69G5FC5";
    let heist_members = [
        ("01ARZ3NDEKTSV4RRFFQ69G5FC1", "01ARZ3NDEKTSV4RRFFQ69G5FD1", "navigator"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FC2", "01ARZ3NDEKTSV4RRFFQ69G5FD2", "insider"),
        ("01ARZ3NDEKTSV4RRFFQ69G5FC3", "01ARZ3NDEKTSV4RRFFQ69G5FD3", "broker"),
    ]
    .into_iter()
    .map(|(member, principal, role)| {
        MembershipV1::new(
            member.parse().map_err(|_| ())?,
            principal.parse().map_err(|_| ())?,
            PrincipalKindV1::Agent,
            MembershipStandingV1::Enabled,
            AccessModeV1::Participant,
            Some(role.to_owned()),
        )
        .map_err(|_| ())
    })
    .collect::<Result<Vec<_>, _>>()?;
    let heist_registry = builtin_worldstream_registry().map_err(|_| ())?;
    let heist_request = PackGenesisRequestV1 {
        room_id: heist_room_id.parse().map_err(|_| ())?,
        pack_digest: agent_heist_digest(),
        configuration: CanonicalJsonV1::parse(br#"{"briefing_duration_seconds":30,"commitment_duration_seconds":30,"commitment_reminder_seconds_before_deadline":10,"maximum_open_offers_per_role":4,"maximum_plans":12,"negotiation_duration_seconds":90,"pack_id":"worldstream.agent-heist","pack_schema":1,"result_duration_seconds":20,"roles":["navigator","insider","broker"]}"#).map_err(|_| ())?,
        room_seed: "hex:101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f"
            .parse()
            .map_err(|_| ())?,
        created_at: "2026-08-21T12:00:00Z".parse().map_err(|_| ())?,
        initial_core_state: CoreRoomStateV1::active(heist_members).map_err(|_| ())?,
    };
    let mut heist_trace = CoreTraceV1::create_from_retained_for_conformance(
        heist_registry
            .prepare_genesis_for_new_room(&heist_request)
            .map_err(|_| ())?,
    )
    .map_err(|_| ())?;
    let initial_timer = heist_trace
        .genesis()
        .initial_timers()
        .first()
        .cloned()
        .ok_or(())?;
    let timer_request = TimerFiredRequestV1::new(
        heist_room_id.parse().map_err(|_| ())?,
        initial_timer.timer_id.clone(),
        initial_timer.generation,
        initial_timer.scheduled_for.clone(),
        initial_timer.canonical_payload.clone(),
    );
    let timer_stimulus = RecordedStimulusV1::TimerFired(TimerFiredV1 {
        timer_id: initial_timer.timer_id.clone(),
        generation: initial_timer.generation,
        scheduled_for: initial_timer.scheduled_for.clone(),
        canonical_payload: initial_timer.canonical_payload.clone(),
    });
    let prepared_transition = heist_trace.prepare(timer_stimulus.clone()).map_err(|_| ())?;
    let authority = PreparedAuthorityWitnessV1::mint_for_conformance(
        "transfer-smoke-heist-timer",
        "01ARZ3NDEKTSV4RRFFQ69G5FD1".parse().map_err(|_| ())?,
        1,
        &CanonicalJsonV1::parse(br#"{"revoked":false,"scope":"timer_fired"}"#)
            .map_err(|_| ())?,
    )
    .map_err(|_| ())?;
    let frame_heads = heist_trace
        .core_state()
        .memberships()
        .keys()
        .cloned()
        .map(|member| (member, 0_u64))
        .collect::<BTreeMap<_, _>>();
    let plan = PreparedRoomCommitV1::for_timer_fired_for_conformance(
        &heist_trace,
        &timer_request,
        prepared_transition,
        "01ARZ3NDEKTSV4RRFFQ69G5FG1".parse().map_err(|_| ())?,
        IntegrityGenerationV1::new(1).map_err(|_| ())?,
        authority,
        &frame_heads,
    )
    .map_err(|_| ())?;
    let mut timers = heist_trace
        .genesis()
        .initial_timers()
        .iter()
        .map(|timer| {
            Ok(SeedTimer {
                timer_id: timer.timer_id.to_string(),
                generation: timer.generation.get(),
                scheduled_for: timer.scheduled_for.as_str().to_owned(),
                payload_bytes: timer.canonical_payload.to_bytes().map_err(|_| ())?,
                state: if timer.timer_id == initial_timer.timer_id
                    && timer.generation == initial_timer.generation
                {
                    "fired"
                } else {
                    "scheduled"
                },
            })
        })
        .collect::<Result<Vec<_>, ()>>()?;
    let PreparedExistingIntentV1::Advance(advance) = plan.intent() else {
        return Err(());
    };
    for mutation in &advance.timer_changes {
        match mutation.kind() {
            PreparedTimerMutationKindV1::Schedule {
                timer_id,
                generation,
                scheduled_for,
                canonical_payload_bytes,
            } => timers.push(SeedTimer {
                timer_id: timer_id.to_string(),
                generation: generation.get(),
                scheduled_for: scheduled_for.as_str().to_owned(),
                payload_bytes: canonical_payload_bytes.to_vec(),
                state: "scheduled",
            }),
            PreparedTimerMutationKindV1::Cancel { timer_id, generation } => {
                timers
                    .iter_mut()
                    .find(|timer| {
                        timer.timer_id == timer_id.to_string()
                            && timer.generation == generation.get()
                    })
                    .ok_or(())?
                    .state = "cancelled";
            }
            PreparedTimerMutationKindV1::Reschedule {
                timer_id,
                previous_generation,
                generation,
                scheduled_for,
                canonical_payload_bytes,
            } => {
                timers
                    .iter_mut()
                    .find(|timer| {
                        timer.timer_id == timer_id.to_string()
                            && timer.generation == previous_generation.get()
                    })
                    .ok_or(())?
                    .state = "cancelled";
                timers.push(SeedTimer {
                    timer_id: timer_id.to_string(),
                    generation: generation.get(),
                    scheduled_for: scheduled_for.as_str().to_owned(),
                    payload_bytes: canonical_payload_bytes.to_vec(),
                    state: "scheduled",
                });
            }
        }
    }
    let timer_receipt = plan.semantic_result().clone();
    let heist_frames = seed_frames(&plan)?;
    heist_trace
        .advance_for_conformance(timer_stimulus)
        .map_err(|_| ())?;
    let heist_room = seed_room_from_trace(
        heist_room_id,
        &heist_trace,
        &["01ARZ3NDEKTSV4RRFFQ69G5FG1"],
        timers,
        heist_frames,
        Some(timer_receipt),
    )?;

    let resource = ResourcePayloadV1::from_bytes(
        ResourceKindV1::Artifact,
        "worldstream.transfer-smoke.fixture",
        b"worldstream exact transfer and restore resource bytes v1\n",
    )
    .map_err(|_| ())?;
    let deployment_identity = worldstream_transfer::DeploymentIdentityV1::new(
        vec![pack_identity(&counter_room)?, pack_identity(&heist_room)?],
        vec![resource.identity().clone()],
    )
    .map_err(|_| ())?;
    let store = SqliteRoomStore::open(path).map_err(|_| ())?;
    store
        .initialize_canonical_metadata("deployment/live-transfer", 7)
        .map_err(|_| ())?;
    store
        .initialize_deployment_identity_with_resources(deployment_identity, vec![resource])
        .map_err(|_| ())?;
    drop(store);

    let connection = Connection::open(path).map_err(|_| ())?;
    seed_room(&connection, &counter_room)?;
    seed_room(&connection, &heist_room)?;
    let mut isolated_room = counter_room.clone();
    isolated_room.room_id = "01ARZ3NDEKTSV4RRFFQ69G5FQ0".to_owned();
    isolated_room.transitions = vec![(
        "01ARZ3NDEKTSV4RRFFQ69G5FQ1".to_owned(),
        isolated_room.transitions.first().ok_or(())?.1.clone(),
    )];
    isolated_room.receipt = None;
    isolated_room.timers.clear();
    isolated_room.integrity_status = "quarantined";
    isolated_room.integrity_generation = 2;
    seed_room(&connection, &isolated_room)?;
    seed_global_authority(
        &connection,
        counter_room_id,
        counter_member_id,
        heist_room_id,
        "01ARZ3NDEKTSV4RRFFQ69G5FC1",
    )?;
    Ok(())
}

fn build_transfer_bundle(
    transfer_id: &str,
    source_evidence: &SourceCanonicalEvidence,
    rows: &worldstream_backup::native_sqlite::NativeSqliteOperationalRowsV1,
) -> Result<(TransferBundleV1, TargetFingerprintV1), ()> {
    let target_backend = postgres_backend_fingerprint().map_err(|_| ())?;
    let lineage_id = source_evidence.lineage_id.clone().ok_or(())?;
    let source_epoch = source_evidence.source_epoch.ok_or(())?;
    let deployment_identity = source_evidence.deployment_identity.clone().ok_or(())?;
    if deployment_identity.packs().is_empty() {
        return Err(());
    }
    let source_backend = BackendFingerprintV1::new(
        BundleProfileV1::SqliteBundled,
        "sqlite-bundled",
        target_backend.schema().clone(),
    )
    .map_err(|_| ())?;
    let spec = NativeSqliteTransferSpecV1::new_with_deployment_resources(
        transfer_id,
        lineage_id,
        source_epoch,
        source_backend,
        target_backend,
        deployment_identity,
        source_evidence.resource_payloads.clone(),
        SessionStatePolicyV1::InvalidateAndRebuild,
    );
    let bundle = NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
        rows,
        &spec,
        &source_evidence.canonical_records,
    )
    .map_err(|_| ())?;
    let target = TargetFingerprintV1::for_bundle(&bundle).map_err(|_| ())?;
    Ok((bundle, target))
}

fn main() {
    let mode = env::var("WORLDSTREAM_PG_TRANSFER_PROVIDER_MODE")
        .unwrap_or_else(|_| "external".to_owned());
    let source = env::var("WORLDSTREAM_PG_TRANSFER_SQLITE").unwrap_or_default();
    let abort_backup =
        env::var("WORLDSTREAM_PG_TRANSFER_ABORT_BACKUP").unwrap_or_default();
    let transfer_backup =
        env::var("WORLDSTREAM_PG_TRANSFER_BACKUP").unwrap_or_default();
    let built_generalized_fixture =
        env::var("WORLDSTREAM_PG_TRANSFER_BUILD_SOURCE").as_deref() == Ok("1");
    let mut evidence = base(&mode);

    let admin_dsn = read_dsn_file("WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE")
        .unwrap_or_else(|_| {
            provider_failure(&mode, "admin_dsn_file_rejected", evidence.clone())
        });
    let runtime_dsn = read_dsn_file("WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE")
        .unwrap_or_else(|_| {
            provider_failure(&mode, "runtime_dsn_file_rejected", evidence.clone())
        });
    let abort_admin_dsn = read_dsn_file("WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE")
        .unwrap_or_else(|_| {
            provider_failure(
                &mode,
                "abort_admin_dsn_file_rejected",
                evidence.clone(),
            )
        });
    if abort_admin_dsn.as_str() == admin_dsn.as_str()
        || abort_admin_dsn.as_str() == runtime_dsn.as_str()
    {
        provider_failure(&mode, "external_abort_target_not_distinct", evidence);
    }

    if built_generalized_fixture && build_disposable_source(Path::new(&source)).is_err()
    {
        incomplete(&mode, "disposable_sqlite_source_build_failed", evidence);
    }

    let source_store = match SqliteRoomStore::open(Path::new(&source)) {
        Ok(store) => store,
        Err(error) => {
            if env::var("WORLDSTREAM_PG_TRANSFER_DEBUG").as_deref() == Ok("1") {
                eprintln!("sqlite_source_lifecycle_open_error: {error:?}");
            }
            incomplete(&mode, "sqlite_source_lifecycle_open_failed", evidence)
        }
    };
    if source_store.source_transfer_state() != SqliteSourceTransferStateV1::SourceAuthoritative {
        incomplete(&mode, "sqlite_source_not_authoritative_before_smoke", evidence);
    }
    let aborted_point = match source_store.begin_source_transfer(Path::new(&abort_backup)) {
        Ok(status) => status,
        Err(_) => incomplete(&mode, "sqlite_source_abort_point_backup_failed", evidence),
    };
    if aborted_point.state() != SqliteSourceTransferStateV1::TransferPending
        || aborted_point.backup_digest().is_none()
    {
        incomplete(&mode, "sqlite_source_abort_point_not_pending", evidence);
    }
    drop(source_store);
    let restarted_source = match SqliteRoomStore::open(Path::new(&source)) {
        Ok(store) => store,
        Err(_) => incomplete(&mode, "sqlite_source_pending_restart_failed", evidence),
    };
    let pending_restart_verified = restarted_source.source_transfer_state()
        == SqliteSourceTransferStateV1::TransferPending
        && restarted_source
            .source_transfer_status()
            .ok()
            .is_some_and(|status| status == aborted_point);
    if !pending_restart_verified {
        incomplete(&mode, "sqlite_source_pending_restart_mismatch", evidence);
    }

    let abort_report = match verify_file(
        Path::new(&abort_backup),
        NativeSqliteLimits::default(),
    ) {
        Ok(report) => report,
        Err(_) => incomplete(&mode, "sqlite_abort_backup_verification_failed", evidence),
    };
    let abort_source_evidence = match extract_source_evidence(
        Path::new(&source),
        Path::new(&abort_backup),
        &abort_report,
    ) {
        Ok(value) if value.missing.is_empty() => value,
        _ => incomplete(&mode, "sqlite_abort_canonical_evidence_incomplete", evidence),
    };
    let abort_rows = match extract_operational_rows(
        Path::new(&abort_backup),
        NativeSqliteLimits::default(),
    ) {
        Ok(rows) => rows,
        Err(_) => incomplete(&mode, "sqlite_abort_operational_extraction_failed", evidence),
    };
    let (abort_bundle, abort_target) = match build_transfer_bundle(
        "live-whole-deployment-abort-probe",
        &abort_source_evidence,
        &abort_rows,
    ) {
        Ok(value) => value,
        Err(_) => incomplete(&mode, "abort_bundle_construction_failed", evidence),
    };

    let success_database = Client::connect(admin_dsn.as_str(), NoTls)
        .ok()
        .and_then(|mut client| client.query_one("SELECT current_database()", &[]).ok())
        .and_then(|row| row.try_get::<_, String>(0).ok());
    let abort_database = Client::connect(abort_admin_dsn.as_str(), NoTls)
        .ok()
        .and_then(|mut client| client.query_one("SELECT current_database()", &[]).ok())
        .and_then(|row| row.try_get::<_, String>(0).ok());
    if success_database.is_none()
        || abort_database.is_none()
        || success_database == abort_database
    {
        provider_failure(&mode, "postgres_abort_target_not_isolated", evidence);
    }
    let abort_admin_config = match PostgresConnectionConfig::direct_admin(
        abort_admin_dsn.as_str().to_owned(),
    ) {
        Ok(config) => config,
        Err(_) => provider_failure(&mode, "abort_admin_dsn_rejected", evidence),
    };
    let abort_admin = match PostgresAdmin::new(abort_admin_config) {
        Ok(admin) => admin,
        Err(_) => provider_failure(&mode, "abort_admin_profile_rejected", evidence),
    };
    if abort_admin.migrate().is_err() || abort_admin.verify_schema().is_err() {
        provider_failure(&mode, "postgres_abort_target_migration_failed", evidence);
    }
    let mut abort_destination = match PostgresTransferDestination::new(
        &abort_admin,
        &abort_bundle,
        abort_target.clone(),
    ) {
        Ok(destination) => destination,
        Err(_) => provider_failure(&mode, "postgres_abort_destination_rejected", evidence),
    };
    let mut abort_session = match TransferImportSessionV1::begin(&abort_bundle) {
        Ok(session) => session,
        Err(_) => incomplete(&mode, "abort_session_begin_failed", evidence),
    };
    if abort_session.verify_target(&abort_target).is_err() {
        incomplete(&mode, "abort_target_epoch_fence_rejected", evidence);
    }
    let abort_chunk_end = abort_bundle.records().len().min(64);
    let abort_chunk = match abort_bundle.chunk(0, abort_chunk_end) {
        Ok(chunk) if abort_chunk_end > 0 => chunk,
        _ => incomplete(&mode, "abort_bundle_has_no_chunk", evidence),
    };

    // Exercise the provider branch where neither staging rows nor a target
    // fence exist yet. A successful abort must still leave one exact durable
    // tombstone, reject a stale publisher, and be idempotent. This probe uses
    // the disposable abort database and removes the verified tombstone only
    // to reset that database for the partial-import coordinator probe below.
    if abort_destination.abort_import(&abort_target).is_err()
        || abort_destination.abort_import(&abort_target).is_err()
    {
        provider_failure(&mode, "postgres_missing_state_abort_failed", evidence);
    }
    if abort_session
        .apply_chunk(&mut abort_destination, &abort_chunk)
        .is_ok()
    {
        provider_failure(
            &mode,
            "postgres_aborted_target_accepted_stale_chunk",
            evidence,
        );
    }
    let (conflicting_abort_bundle, conflicting_abort_target) = match build_transfer_bundle(
        "live-whole-deployment-conflicting-abort-probe",
        &abort_source_evidence,
        &abort_rows,
    ) {
        Ok(value) => value,
        Err(_) => incomplete(&mode, "conflicting_abort_bundle_construction_failed", evidence),
    };
    if abort_destination
        .abort_import(&conflicting_abort_target)
        .is_ok()
    {
        provider_failure(&mode, "postgres_abort_wrong_target_accepted", evidence);
    }
    let mut conflicting_abort_destination = match PostgresTransferDestination::new(
        &abort_admin,
        &conflicting_abort_bundle,
        conflicting_abort_target.clone(),
    ) {
        Ok(destination) => destination,
        Err(_) => provider_failure(&mode, "conflicting_abort_destination_rejected", evidence),
    };
    if conflicting_abort_destination
        .abort_import(&conflicting_abort_target)
        .is_ok()
    {
        provider_failure(&mode, "postgres_abort_wrong_bundle_accepted", evidence);
    }
    drop(conflicting_abort_destination);

    let abort_bundle_hash = match abort_bundle.bundle_hash() {
        Ok(hash) => hash,
        Err(_) => incomplete(&mode, "abort_bundle_hash_failed", evidence),
    };
    let abort_target_digest = match abort_target.fingerprint_digest() {
        Ok(digest) => digest,
        Err(_) => incomplete(&mode, "abort_target_digest_failed", evidence),
    };
    let abort_bundle_key = abort_bundle_hash.as_bytes().to_vec();
    let abort_target_key = abort_target_digest.as_bytes().to_vec();
    let missing_state_tombstone_verified = Client::connect(abort_admin_dsn.as_str(), NoTls)
        .ok()
        .and_then(|mut client| {
            client
                .query_one(
                    "SELECT bundle_hash, target_fingerprint, state, \
                     (SELECT count(*)::bigint FROM worldstream_transfer_imports), \
                     (SELECT count(*)::bigint FROM worldstream_transfer_chunks) \
                     FROM worldstream_transfer_target_fence WHERE fence_id = true",
                    &[],
                )
                .ok()
        })
        .and_then(|row| {
            Some((
                row.try_get::<_, Vec<u8>>(0).ok()?,
                row.try_get::<_, Vec<u8>>(1).ok()?,
                row.try_get::<_, String>(2).ok()?,
                row.try_get::<_, i64>(3).ok()?,
                row.try_get::<_, i64>(4).ok()?,
            ))
        })
        == Some((
            abort_bundle_key.clone(),
            abort_target_key.clone(),
            "aborted".to_owned(),
            0,
            0,
        ));
    if !missing_state_tombstone_verified {
        provider_failure(
            &mode,
            "postgres_missing_state_abort_tombstone_incomplete",
            evidence,
        );
    }
    let missing_state_probe_reset = Client::connect(abort_admin_dsn.as_str(), NoTls)
        .ok()
        .and_then(|mut client| {
            client
                .execute(
                    "DELETE FROM worldstream_transfer_target_fence \
                     WHERE fence_id = true AND bundle_hash = $1 \
                     AND target_fingerprint = $2 AND state = 'aborted'",
                    &[&abort_bundle_key.as_slice(), &abort_target_key.as_slice()],
                )
                .ok()
        })
        == Some(1);
    if !missing_state_probe_reset {
        provider_failure(&mode, "postgres_missing_state_abort_probe_reset_failed", evidence);
    }

    if abort_session
        .apply_chunk(&mut abort_destination, &abort_chunk)
        .is_err()
    {
        provider_failure(&mode, "postgres_abort_probe_chunk_failed", evidence);
    }
    let pre_abort_checkpoint = match abort_session.to_bytes() {
        Ok(bytes) => bytes,
        Err(_) => incomplete(&mode, "abort_checkpoint_serialize_failed", evidence),
    };
    if let Err(error) = abort_whole_deployment(
        &mut abort_session,
        &mut abort_destination,
        &restarted_source,
        &abort_bundle,
    ) {
        if env::var("WORLDSTREAM_PG_TRANSFER_DEBUG").as_deref() == Ok("1") {
            eprintln!("coordinated_abort_error: {error:?}");
        }
        provider_failure(&mode, "coordinated_abort_failed", evidence);
    }
    let abort_restored_authority = abort_session.state()
        == TransferStateV1::SourceAuthoritative
        && restarted_source.source_transfer_state()
            == SqliteSourceTransferStateV1::SourceAuthoritative;
    if !abort_restored_authority {
        provider_failure(&mode, "coordinated_abort_did_not_restore_source", evidence);
    }
    drop(restarted_source);
    let restarted_after_abort = match SqliteRoomStore::open(Path::new(&source)) {
        Ok(store) => store,
        Err(_) => incomplete(&mode, "sqlite_source_abort_restart_failed", evidence),
    };
    let mut resumed_abort = match TransferImportSessionV1::from_bytes(
        &abort_bundle,
        &pre_abort_checkpoint,
    ) {
        Ok(session) => session,
        Err(_) => incomplete(&mode, "abort_checkpoint_resume_failed", evidence),
    };
    if abort_whole_deployment(
        &mut resumed_abort,
        &mut abort_destination,
        &restarted_after_abort,
        &abort_bundle,
    )
    .is_err()
    {
        provider_failure(&mode, "coordinated_abort_retry_failed", evidence);
    }
    let abort_idempotence_verified = resumed_abort.state()
        == TransferStateV1::SourceAuthoritative
        && restarted_after_abort.source_transfer_state()
            == SqliteSourceTransferStateV1::SourceAuthoritative;
    let abort_provider_tombstone_verified = Client::connect(abort_admin_dsn.as_str(), NoTls)
        .ok()
        .and_then(|mut client| {
            client
                .query_one(
                    "SELECT state, \
                     (SELECT count(*)::bigint FROM worldstream_transfer_imports), \
                     (SELECT count(*)::bigint FROM worldstream_transfer_chunks) \
                     FROM worldstream_transfer_target_fence WHERE fence_id = true",
                    &[],
                )
                .ok()
        })
        .and_then(|row| {
            Some((
                row.try_get::<_, String>(0).ok()?,
                row.try_get::<_, i64>(1).ok()?,
                row.try_get::<_, i64>(2).ok()?,
            ))
        })
        == Some(("aborted".to_owned(), 0, 0));
    if !abort_idempotence_verified || !abort_provider_tombstone_verified {
        provider_failure(&mode, "coordinated_abort_evidence_incomplete", evidence);
    }
    drop(abort_destination);

    let transfer_point = match restarted_after_abort
        .begin_source_transfer(Path::new(&transfer_backup))
    {
        Ok(status) => status,
        Err(_) => incomplete(&mode, "sqlite_source_transfer_point_backup_failed", evidence),
    };
    if transfer_point.state() != SqliteSourceTransferStateV1::TransferPending
        || transfer_point.backup_digest().is_none()
    {
        incomplete(&mode, "sqlite_source_transfer_point_not_pending", evidence);
    }
    drop(restarted_after_abort);
    let source_store = match SqliteRoomStore::open(Path::new(&source)) {
        Ok(store) => store,
        Err(_) => incomplete(&mode, "sqlite_source_transfer_restart_failed", evidence),
    };
    let transfer_pending_restart_verified = source_store.source_transfer_state()
        == SqliteSourceTransferStateV1::TransferPending
        && source_store
            .source_transfer_status()
            .ok()
            .is_some_and(|status| status == transfer_point);
    if !transfer_pending_restart_verified {
        incomplete(&mode, "sqlite_source_transfer_restart_mismatch", evidence);
    }

    let source_report = match verify_file(
        Path::new(&transfer_backup),
        NativeSqliteLimits::default(),
    ) {
        Ok(report) => report,
        Err(_) => incomplete(&mode, "sqlite_transfer_backup_verification_failed", evidence),
    };
    evidence["source"] = json!({
        "status": if source_report.canonical_ready { "canonical_ready" } else { "not_canonical_ready" },
        "canonical_ready": source_report.canonical_ready,
        "engine_version": source_report.engine_version,
        "query_only": source_report.query_only,
        "room_count": source_report.room_count,
        "transition_count": source_report.transition_count,
        "snapshot_count": source_report.snapshot_count,
        "operational_counts": {
            "timers": source_report.timer_count,
            "frames": source_report.frame_count,
            "semantic_receipts": source_report.semantic_receipt_count,
            "activation_intents": source_report.activation_intent_count,
            "activation_receipts": source_report.activation_receipt_count,
            "activation_decisions": source_report.activation_decision_count,
            "observation_consequences": source_report.observation_consequence_count
        },
        "native_diagnostics": source_report.diagnostics.iter().map(|diagnostic| json!({
            "code": diagnostic.code,
            "blocking": diagnostic.blocking,
            "subject": redacted_subject(&diagnostic.subject),
            "action": diagnostic.action
        })).collect::<Vec<_>>()
    });

    let source_evidence = match extract_source_evidence(
        Path::new(&source),
        Path::new(&transfer_backup),
        &source_report,
    ) {
        Ok(value) => value,
        Err(_) => incomplete(&mode, "sqlite_canonical_evidence_extraction_failed", evidence),
    };
    // The transfer contract is gated by the authenticated SQLite canonical
    // export, its authoritative DeploymentIdentityV1 record, and the exact
    // source-owned backup witness. Native-restore readiness remains a separate
    // diagnostic because an already quarantined Room may intentionally retain
    // corrupt bytes that must transfer without promotion.
    let source_complete = source_evidence.missing.is_empty();
    let source_restore_metadata_complete = source_evidence.metadata_missing.is_empty();
    let mut source_missing = source_evidence.missing.clone();
    source_missing.extend(source_evidence.metadata_missing.clone());
    evidence["source"]["canonical_evidence"] = json!({
        "status": if source_complete { "complete" } else { "incomplete" },
        "stored_bytes": source_evidence.stored_bytes,
        "rooms": source_evidence.rooms,
        "missing_evidence": source_missing,
        "metadata": {
            "status": if source_restore_metadata_complete { "complete" } else { "incomplete" },
            "diagnostic": "independent_native_backup_restore_readiness",
            "authoritative_for_transfer": false,
            "required_for_transfer": false,
            "missing_evidence": source_evidence.metadata_missing.clone(),
        },
        "resources": {
            "status": if source_evidence.deployment_identity.is_some() { "complete" } else { "missing" },
            "count": source_evidence.resources.len(),
            "bytes_verified": source_evidence.resource_payloads.len() == source_evidence.resources.len(),
        },
        "room_pack_lock_identity_observed": source_evidence.packs.len(),
        "canonical_transition_count": source_evidence.transition_count,
        "isolated_room_count": source_evidence.isolated_rooms.len(),
        "isolated_room_subjects": source_evidence
            .isolated_rooms
            .iter()
            .map(|room_id| redacted_subject(room_id))
            .collect::<Vec<_>>(),
    });
    if !source_complete {
        incomplete(&mode, "sqlite_source_canonical_evidence_incomplete", evidence);
    }
    if let Some(source) = evidence.get_mut("source").and_then(Value::as_object_mut) {
        source.insert(
            "transfer_contract_gate".to_owned(),
            json!({
                "name": "authoritative_deployment_identity_v1",
                "status": if source_evidence.deployment_identity.is_some() { "observed" } else { "missing" },
                "source_authority": "sqlite_canonical_export",
                "membership_required": true,
                "backup_diagnostic_authoritative": false,
                "backup_readiness_required": false
            }),
        );
        source.insert(
            "backup_diagnostic".to_owned(),
            json!({
                "canonical_ready": source_report.canonical_ready,
                "status": if source_report.canonical_ready { "ready" } else { "not_ready" },
                "diagnostic": "independent_native_backup_restore_readiness",
                "authoritative_for_transfer": false,
                "required_for_transfer": false
            }),
        );
    }

    let mut version_client = match Client::connect(admin_dsn.as_str(), NoTls) {
        Ok(client) => client,
        Err(_) => provider_failure(&mode, "postgres_admin_connection_failed", evidence),
    };
    let server_version_num: String = match version_client.query_one("SHOW server_version_num", &[]) {
        Ok(row) => match row.try_get(0) {
            Ok(version) => version,
            Err(_) => provider_failure(&mode, "postgres_version_probe_failed", evidence),
        },
        Err(_) => provider_failure(&mode, "postgres_version_probe_failed", evidence),
    };
    let parsed_version = server_version_num.parse::<u32>().unwrap_or_default();
    let major = parsed_version / 10_000;
    let patch = parsed_version % 10_000;
    if major != 17 || patch < 11 {
        wrong_version(&mode, &server_version_num, evidence);
    }
    evidence["postgres"] = json!({
        "required": "17.11",
        "server_version_num": server_version_num,
        "major": major,
        "patch": patch,
        "status": "version_verified"
    });
    drop(version_client);

    let rows = match extract_operational_rows(
        Path::new(&transfer_backup),
        NativeSqliteLimits::default(),
    ) {
        Ok(rows) => rows,
        Err(_) => incomplete(&mode, "sqlite_operational_extraction_failed", evidence),
    };
    let operational_row_count: usize = rows.tables.values().map(Vec::len).sum();
    let source_membership_row_count = rows.tables.get("room_members").map_or(0, Vec::len);
    let source_semantic_receipt_count = rows
        .tables
        .get("semantic_receipts")
        .map_or(0, Vec::len);
    let source_identity_pack_count = source_evidence
        .deployment_identity
        .as_ref()
        .map_or(0, |identity| identity.packs().len());
    let source_identity_resource_count = source_evidence
        .deployment_identity
        .as_ref()
        .map_or(0, |identity| identity.resources().len());
    let source_identity_witness = source_evidence.deployment_identity.is_some()
        && source_identity_pack_count >= 2
        && source_evidence.packs.len() == source_identity_pack_count
        && source_identity_resource_count > 0
        && source_evidence.resource_payloads.len() == source_identity_resource_count;
    let source_membership_witness = source_membership_row_count > 0;
    if !source_identity_witness {
        incomplete(&mode, "source_authoritative_deployment_identity_missing", evidence);
    }
    if !source_membership_witness {
        incomplete(&mode, "source_membership_witness_missing", evidence);
    }
    if let Some(source) = evidence.get_mut("source").and_then(Value::as_object_mut) {
        source.insert(
            "transfer_source_witness".to_owned(),
            json!({
                "authority": "sqlite_canonical_export_and_native_operational_rows",
                "deployment_identity": if source_identity_witness { "complete" } else { "missing" },
                "pack_count": source_identity_pack_count,
                "resource_count": source_identity_resource_count,
                "membership": if source_membership_witness { "complete" } else { "missing" },
                "membership_row_count": source_membership_row_count,
                "backup_diagnostic_can_mask_transfer": false
            }),
        );
    }
    let target_backend = match postgres_backend_fingerprint() {
        Ok(value) => value,
        Err(_) => provider_failure(&mode, "postgres_backend_fingerprint_failed", evidence),
    };
    let lineage_id = match source_evidence.lineage_id.clone() {
        Some(value) => value,
        None => incomplete(&mode, "source_deployment_lineage_missing", evidence),
    };
    let source_epoch = match source_evidence.source_epoch {
        Some(value) => value,
        None => incomplete(&mode, "source_storage_epoch_missing", evidence),
    };
    let deployment_identity = match source_evidence.deployment_identity.clone() {
        Some(identity) => identity,
        None => incomplete(&mode, "source_deployment_identity_missing", evidence),
    };
    let source_backend = match BackendFingerprintV1::new(
        BundleProfileV1::SqliteBundled,
        "sqlite-bundled",
        target_backend.schema().clone(),
    ) {
        Ok(value) => value,
        Err(_) => incomplete(&mode, "sqlite_backend_fingerprint_failed", evidence),
    };
    if deployment_identity.packs().is_empty() {
        incomplete(&mode, "source_deployment_pack_identity_missing", evidence);
    }
    let spec = NativeSqliteTransferSpecV1::new_with_deployment_resources(
        "live-whole-deployment-transfer",
        lineage_id,
        source_epoch,
        source_backend,
        target_backend,
        deployment_identity,
        source_evidence.resource_payloads.clone(),
        SessionStatePolicyV1::InvalidateAndRebuild,
    );
    let bundle = match NativeSqliteTransferAdapterV1::from_operational_rows_with_canonical_records(
        &rows,
        &spec,
        &source_evidence.canonical_records,
    ) {
        Ok(bundle) => bundle,
        Err(_) => incomplete(&mode, "whole_deployment_bundle_construction_failed", evidence),
    };
    let target = match TargetFingerprintV1::for_bundle(&bundle) {
        Ok(target) => target,
        Err(_) => incomplete(&mode, "target_fingerprint_failed", evidence),
    };
    let bundle_hash = match bundle.bundle_hash() {
        Ok(hash) => hash,
        Err(_) => incomplete(&mode, "bundle_hash_failed", evidence),
    };
    let target_fingerprint_digest = match target.fingerprint_digest() {
        Ok(digest) => digest,
        Err(_) => incomplete(&mode, "target_fingerprint_digest_failed", evidence),
    };

    let admin_config = match PostgresConnectionConfig::direct_admin(
        admin_dsn.as_str().to_owned(),
    ) {
        Ok(config) => config,
        Err(_) => provider_failure(&mode, "admin_dsn_rejected", evidence),
    };
    let admin = match PostgresAdmin::new(admin_config) {
        Ok(admin) => admin,
        Err(_) => provider_failure(&mode, "admin_profile_rejected", evidence),
    };
    if let Err(error) = admin.migrate() {
        if env::var("WORLDSTREAM_PG_TRANSFER_DEBUG").as_deref() == Ok("1") {
            eprintln!("postgres_admin_migration_error: {error:?}");
        }
        provider_failure(&mode, "postgres_admin_migration_failed", evidence);
    }
    let runtime_migration_privilege_role = if mode == "docker" {
        Some("runtime".to_owned())
    } else {
        env::var("WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE")
            .ok()
            .filter(|role| !role.is_empty())
    };
    if let Some(runtime_role) = runtime_migration_privilege_role {
        if !runtime_role
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            provider_failure(&mode, "postgres_runtime_role_identifier_rejected", evidence);
        }
        let mut privilege_client = match Client::connect(
            admin_dsn.as_str(),
            NoTls,
        ) {
            Ok(client) => client,
            Err(_) => provider_failure(&mode, "postgres_privilege_connection_failed", evidence),
        };
        if privilege_client
            .batch_execute(&format!(
                "REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON TABLE \
                 public.worldstream_schema_migrations, \
                 public.worldstream_transfer_imports, \
                 public.worldstream_transfer_chunks, \
                 public.worldstream_transfer_target_fence FROM {runtime_role}",
            ))
            .is_err()
        {
            provider_failure(&mode, "postgres_runtime_migration_privilege_revoke_failed", evidence);
        }
    }
    if admin.verify_schema().is_err() {
        provider_failure(&mode, "postgres_admin_read_only_verification_failed", evidence);
    }
    let runtime_config = match PostgresConnectionConfig::runtime(
        runtime_dsn.as_str().to_owned(),
        PostgresConnectionPath::Direct,
    ) {
        Ok(config) => config,
        Err(_) => provider_failure(&mode, "runtime_dsn_rejected", evidence),
    };
    let store = match PostgresRoomStore::new(runtime_config) {
        Ok(store) => store,
        Err(_) => provider_failure(&mode, "runtime_profile_rejected", evidence),
    };
    if let Err(error) = store.verify_schema() {
        if env::var("WORLDSTREAM_PG_TRANSFER_DEBUG").as_deref() == Ok("1") {
            eprintln!("runtime_read_only_schema_error: {error:?}");
        }
        provider_failure(&mode, "runtime_read_only_schema_verification_failed", evidence);
    }
    let runtime_transfer_control_denied = if mode == "docker" {
        Client::connect(
            runtime_dsn.as_str(),
            NoTls,
        )
        .ok()
        .is_some_and(|mut client| {
            client
                .execute(
                    "DELETE FROM public.worldstream_transfer_target_fence WHERE fence_id = true",
                    &[],
                )
                .is_err()
        })
    } else {
        true
    };
    if !runtime_transfer_control_denied {
        provider_failure(&mode, "postgres_runtime_transfer_control_not_denied", evidence);
    }
    let mut destination = match PostgresTransferDestination::new(&admin, &bundle, target.clone()) {
        Ok(destination) => destination,
        Err(_) => provider_failure(&mode, "postgres_destination_fence_rejected", evidence),
    };
    let mut session = match TransferImportSessionV1::begin(&bundle) {
        Ok(session) => session,
        Err(_) => incomplete(&mode, "transfer_session_begin_failed", evidence),
    };
    if session.verify_target(&target).is_err() {
        incomplete(&mode, "transfer_target_epoch_fence_rejected", evidence);
    }

    let chunk_size = 64_usize;
    let chunk_count = bundle.records().len().div_ceil(chunk_size);
    let mut chunks = Vec::with_capacity(chunk_count);
    let mut start = 0_usize;
    while start < bundle.records().len() {
        let end = (start + chunk_size).min(bundle.records().len());
        chunks.push(match bundle.chunk(start, end) {
            Ok(chunk) => chunk,
            Err(_) => incomplete(&mode, "transfer_chunk_build_failed", evidence),
        });
        start = end;
    }
    let first_chunk = match chunks.first() {
        Some(chunk) => chunk,
        None => incomplete(&mode, "transfer_bundle_has_no_chunks", evidence),
    };
    let deployment_lineage_bytes = match bundle.records().iter().find_map(|record| {
        matches!(
            record.kind(),
            RecordKindV1::Canonical(CanonicalRecordKindV1::DeploymentLineage)
        )
        .then(|| record.bytes().to_vec())
    }) {
        Some(bytes) => bytes,
        None => incomplete(&mode, "transfer_lineage_record_missing", evidence),
    };
    let storage_epoch_bytes = match bundle.records().iter().find_map(|record| {
        matches!(
            record.kind(),
            RecordKindV1::Canonical(CanonicalRecordKindV1::StorageEpoch)
        )
        .then(|| record.bytes().to_vec())
    }) {
        Some(bytes) => bytes,
        None => incomplete(&mode, "transfer_epoch_record_missing", evidence),
    };
    let target_epoch = match i64::try_from(target.storage_epoch()) {
        Ok(epoch) => epoch,
        Err(_) => incomplete(&mode, "transfer_target_epoch_out_of_range", evidence),
    };
    let mut preseed_client = match Client::connect(
        admin_dsn.as_str(),
        NoTls,
    ) {
        Ok(client) => client,
        Err(_) => provider_failure(&mode, "postgres_preseed_connection_failed", evidence),
    };
    let mut exact_preseed_destination = match PostgresTransferDestination::new(
        &admin,
        &bundle,
        target.clone(),
    ) {
        Ok(destination) => destination,
        Err(_) => provider_failure(&mode, "postgres_exact_preseed_destination_failed", evidence),
    };
    if preseed_client
        .execute(
            "INSERT INTO worldstream_deployment_metadata(\
             target_id, deployment_lineage_bytes, storage_epoch_bytes, storage_epoch\
             ) VALUES (true, $1, $2, $3)",
            &[&deployment_lineage_bytes, &storage_epoch_bytes, &target_epoch],
        )
        .is_err()
    {
        provider_failure(&mode, "postgres_exact_preseed_setup_failed", evidence);
    }
    let exact_preseed_rejected = TransferImportSessionV1::begin(&bundle)
        .ok()
        .is_some_and(|mut preseed_session| {
            preseed_session.verify_target(&target).is_ok()
                && preseed_session
                    .apply_chunk(&mut exact_preseed_destination, first_chunk)
                    .is_err()
        });
    let exact_preseed_rollback_clean = preseed_client
        .query_one(
            "SELECT \
             (SELECT count(*)::bigint FROM worldstream_transfer_target_fence), \
             (SELECT count(*)::bigint FROM worldstream_transfer_imports), \
             (SELECT count(*)::bigint FROM worldstream_transfer_chunks)",
            &[],
        )
        .ok()
        .and_then(|row| {
            Some((
                row.try_get::<_, i64>(0).ok()?,
                row.try_get::<_, i64>(1).ok()?,
                row.try_get::<_, i64>(2).ok()?,
            ))
        })
        == Some((0, 0, 0));
    if !exact_preseed_rejected || !exact_preseed_rollback_clean {
        provider_failure(&mode, "postgres_exact_preseed_was_not_rejected", evidence);
    }
    if preseed_client
        .execute(
            "DELETE FROM worldstream_deployment_metadata WHERE target_id = true",
            &[],
        )
        .ok()
        != Some(1)
    {
        provider_failure(&mode, "postgres_exact_preseed_cleanup_failed", evidence);
    }

    let conflicting_lineage_bytes = b"deployment/conflicting-preseed".to_vec();
    let mut conflicting_preseed_destination = match PostgresTransferDestination::new(
        &admin,
        &bundle,
        target.clone(),
    ) {
        Ok(destination) => destination,
        Err(_) => provider_failure(&mode, "postgres_conflicting_preseed_destination_failed", evidence),
    };
    if preseed_client
        .execute(
            "INSERT INTO worldstream_deployment_metadata(\
             target_id, deployment_lineage_bytes, storage_epoch_bytes, storage_epoch\
             ) VALUES (true, $1, $2, $3)",
            &[&conflicting_lineage_bytes, &storage_epoch_bytes, &target_epoch],
        )
        .is_err()
    {
        provider_failure(&mode, "postgres_conflicting_preseed_setup_failed", evidence);
    }
    let conflicting_preseed_rejected = TransferImportSessionV1::begin(&bundle)
        .ok()
        .is_some_and(|mut preseed_session| {
            preseed_session.verify_target(&target).is_ok()
                && preseed_session
                    .apply_chunk(&mut conflicting_preseed_destination, first_chunk)
                    .is_err()
        });
    let conflicting_preseed_rollback_clean = preseed_client
        .query_one(
            "SELECT \
             (SELECT count(*)::bigint FROM worldstream_transfer_target_fence), \
             (SELECT count(*)::bigint FROM worldstream_transfer_imports), \
             (SELECT count(*)::bigint FROM worldstream_transfer_chunks)",
            &[],
        )
        .ok()
        .and_then(|row| {
            Some((
                row.try_get::<_, i64>(0).ok()?,
                row.try_get::<_, i64>(1).ok()?,
                row.try_get::<_, i64>(2).ok()?,
            ))
        })
        == Some((0, 0, 0));
    if !conflicting_preseed_rejected || !conflicting_preseed_rollback_clean {
        provider_failure(&mode, "postgres_conflicting_preseed_was_not_rejected", evidence);
    }
    if preseed_client
        .execute(
            "DELETE FROM worldstream_deployment_metadata WHERE target_id = true",
            &[],
        )
        .ok()
        != Some(1)
    {
        provider_failure(&mode, "postgres_conflicting_preseed_cleanup_failed", evidence);
    }
    drop(preseed_client);

    let mut interrupted_destination = match PostgresTransferDestination::new(&admin, &bundle, target.clone()) {
        Ok(destination) => destination,
        Err(_) => provider_failure(&mode, "postgres_interruption_destination_fence_rejected", evidence),
    };
    let mut interrupted = match TransferImportSessionV1::begin(&bundle) {
        Ok(session) => session,
        Err(_) => incomplete(&mode, "transfer_interruption_session_begin_failed", evidence),
    };
    if interrupted.verify_target(&target).is_err() {
        incomplete(&mode, "transfer_interruption_target_epoch_fence_rejected", evidence);
    }
    let interruption_disposition = match chunks.first() {
        Some(chunk) => match interrupted.apply_chunk(&mut interrupted_destination, chunk) {
            Ok(disposition) => format!("{disposition:?}").to_lowercase(),
            Err(_) => provider_failure(&mode, "postgres_interruption_chunk_failed", evidence),
        },
        None => incomplete(&mode, "transfer_bundle_has_no_chunks", evidence),
    };
    let conflicting_chunk_rejected = match chunks.first() {
        Some(chunk) => {
            let record = chunk.records().first().ok_or(()).and_then(|record| {
                let RecordKindV1::Canonical(kind) = record.kind() else {
                    return Err(());
                };
                let mut bytes = record.bytes().to_vec();
                if let Some(first) = bytes.first_mut() {
                    *first ^= 1;
                }
                worldstream_transfer::LogicalRecordV1::canonical(
                    0,
                    kind,
                    record.identity(),
                    &bytes,
                )
                .map_err(|_| ())
            });
            match record {
                Ok(record) => match TransferChunkV1::from_records(
                    chunk.bundle_hash(),
                    chunk.start(),
                    vec![record],
                ) {
                    Ok(conflicting) => interrupted_destination.apply_chunk(&conflicting).is_err(),
                    Err(_) => false,
                },
                Err(_) => false,
            }
        }
        None => false,
    };
    if !conflicting_chunk_rejected {
        incomplete(&mode, "postgres_conflicting_chunk_was_accepted", evidence);
    }
    // Drop the first in-memory session without aborting its durable provider
    // state. The next session must attach to that exact fenced import and
    // confirm the already-applied chunk, which models a process restart.
    drop(interrupted);
    drop(interrupted_destination);
    let first_disposition = match chunks.first() {
        Some(chunk) => match session.apply_chunk(&mut destination, chunk) {
            Ok(disposition) => format!("{disposition:?}").to_lowercase(),
            Err(_) => provider_failure(&mode, "postgres_first_chunk_failed", evidence),
        },
        None => incomplete(&mode, "transfer_bundle_has_no_chunks", evidence),
    };
    let checkpoint = match session.to_bytes() {
        Ok(bytes) => bytes,
        Err(_) => incomplete(&mode, "transfer_checkpoint_serialize_failed", evidence),
    };
    let mut resumed = match TransferImportSessionV1::from_bytes(&bundle, &checkpoint) {
        Ok(session) => session,
        Err(_) => incomplete(&mode, "transfer_checkpoint_resume_failed", evidence),
    };
    let replay_disposition = match chunks.first() {
        Some(chunk) => match resumed.apply_chunk(&mut destination, chunk) {
            Ok(disposition) => format!("{disposition:?}").to_lowercase(),
            Err(_) => provider_failure(&mode, "postgres_chunk_replay_failed", evidence),
        },
        None => incomplete(&mode, "transfer_bundle_has_no_chunks", evidence),
    };
    if chunks.len() > 1 {
        for chunk in &chunks[1..] {
            if resumed.apply_chunk(&mut destination, chunk).is_err() {
                provider_failure(&mode, "postgres_resumed_chunk_failed", evidence);
            }
        }
    }
    if !resumed.is_complete() || resumed.state() != TransferStateV1::TargetVerified {
        provider_failure(&mode, "transfer_checkpoint_not_complete", evidence);
    }
    if let Err(error) = finalize_whole_deployment(
        &mut resumed,
        &mut destination,
        &source_store,
        &bundle,
    ) {
        if env::var("WORLDSTREAM_PG_TRANSFER_DEBUG").as_deref() == Ok("1") {
            eprintln!("whole_deployment_finalization_error: {error:?}");
        }
        provider_failure(&mode, "whole_deployment_finalization_failed", evidence);
    }
    let source_retired = source_store
        .source_transfer_status()
        .ok()
        .is_some_and(|status| {
            status.state() == SqliteSourceTransferStateV1::SourceRetired
                && status.bundle_hash() == Some(bundle_hash)
                && status.target_fingerprint() == Some(target_fingerprint_digest)
        })
        && resumed.state() == TransferStateV1::TargetAuthoritative;
    if !source_retired {
        provider_failure(&mode, "sqlite_source_retirement_failed", evidence);
    }
    let finalized_checkpoint = match resumed.to_bytes() {
        Ok(bytes) => bytes,
        Err(_) => incomplete(&mode, "finalized_checkpoint_serialize_failed", evidence),
    };
    drop(source_store);
    let retired_source = match SqliteRoomStore::open(Path::new(&source)) {
        Ok(store) => store,
        Err(_) => provider_failure(&mode, "sqlite_retired_source_restart_failed", evidence),
    };
    let mut resumed_finalization = match TransferImportSessionV1::from_bytes(
        &bundle,
        &finalized_checkpoint,
    ) {
        Ok(session) => session,
        Err(_) => incomplete(&mode, "finalized_checkpoint_resume_failed", evidence),
    };
    let retirement_abort_refused = abort_whole_deployment(
        &mut resumed_finalization,
        &mut destination,
        &retired_source,
        &bundle,
    )
    .is_err();
    let source_retirement_restart_verified = retired_source.source_transfer_state()
        == SqliteSourceTransferStateV1::SourceRetired
        && retirement_abort_refused;
    if !source_retirement_restart_verified {
        provider_failure(&mode, "sqlite_retired_source_was_not_irreversible", evidence);
    }
    if let Err(error) = finalize_whole_deployment(
        &mut resumed_finalization,
        &mut destination,
        &retired_source,
        &bundle,
    ) {
        if env::var("WORLDSTREAM_PG_TRANSFER_DEBUG").as_deref() == Ok("1") {
            eprintln!("whole_deployment_finalization_retry_error: {error:?}");
        }
        provider_failure(&mode, "whole_deployment_finalization_retry_failed", evidence);
    }
    let finalization_restart_idempotence_verified =
        resumed_finalization.state() == TransferStateV1::TargetAuthoritative;
    if !finalization_restart_idempotence_verified {
        provider_failure(&mode, "whole_deployment_finalization_retry_incomplete", evidence);
    }
    drop(retired_source);

    // Read the published Core-owned rows back through the provider's public
    // least-privileged verifier. Every healthy Room's Genesis, Head,
    // materializations, exact pack lock, Membership, operational ledgers, and
    // Transition sequence must round-trip after the authority transition.
    let target_room_count = source_evidence.canonical_rooms.len();
    let mut verified_room_count = 0usize;
    let mut read_client = match Client::connect(
        runtime_dsn.as_str(),
        NoTls,
    ) {
        Ok(client) => client,
        Err(_) => provider_failure(&mode, "runtime_read_only_connection_failed", evidence),
    };
    for room_id in source_evidence.canonical_rooms.keys() {
        let Some(source_room) = source_evidence.canonical_rooms.get(room_id) else {
            provider_failure(&mode, "source_room_readback_index_failed", evidence);
        };
        if source_evidence.isolated_rooms.contains(room_id) {
            let row = match read_client.query_one(
                "SELECT g.pack_revision_lock_bytes, g.genesis_bytes, r.head_bytes, m.core_state_bytes, m.activity_state_bytes, r.integrity_status FROM worldstream_room_roots r JOIN worldstream_genesis g ON g.room_id = r.room_id JOIN worldstream_materializations m ON m.room_id = r.room_id WHERE r.room_id = $1",
                &[room_id],
            ) {
                Ok(row) => row,
                Err(_) => provider_failure(&mode, "postgres_isolated_room_readback_failed", evidence),
            };
            let transitions = match read_client.query(
                "SELECT transition_bytes FROM worldstream_transitions WHERE room_id = $1 ORDER BY room_seq",
                &[room_id],
            ) {
                Ok(rows) => rows
                    .into_iter()
                    .map(|row| row.try_get::<_, Vec<u8>>(0).map_err(|_| ()))
                    .collect::<Result<Vec<_>, _>>(),
                Err(_) => Err(()),
            };
            let exact = row.try_get::<_, Vec<u8>>(0).ok().as_deref()
                == Some(source_room.pack_revision_lock_bytes.as_slice())
                && row.try_get::<_, Vec<u8>>(1).ok().as_deref()
                    == Some(source_room._genesis_bytes.as_slice())
                && row.try_get::<_, Vec<u8>>(2).ok().as_deref()
                    == Some(source_room._head_bytes.as_slice())
                && row.try_get::<_, Vec<u8>>(3).ok().as_deref()
                    == Some(source_room._core_state_bytes.as_slice())
                && row.try_get::<_, Vec<u8>>(4).ok().as_deref()
                    == Some(source_room._activity_state_bytes.as_slice())
                && row
                    .try_get::<_, String>(5)
                    .ok()
                    .is_some_and(|status| status != "healthy")
                && transitions.as_ref().is_ok_and(|bytes| bytes == &source_room.transition_bytes);
            if !exact {
                provider_failure(&mode, "postgres_isolated_room_byte_parity_failed", evidence);
            }
            verified_room_count = verified_room_count.saturating_add(1);
            continue;
        }
        let verification = match store.verify_room(room_id) {
            Ok(value) => value,
            Err(_) => provider_failure(&mode, "postgres_room_readback_failed", evidence),
        };
        if verification.pack_revision_lock_bytes != source_room.pack_revision_lock_bytes
            || verification.genesis_bytes != source_room._genesis_bytes
            || verification.head_bytes != source_room._head_bytes
            || verification.core_state_bytes != source_room._core_state_bytes
            || verification.activity_state_bytes != source_room._activity_state_bytes
            || verification.transition_bytes != source_room.transition_bytes
        {
            provider_failure(&mode, "postgres_room_byte_parity_failed", evidence);
        }
        verified_room_count = verified_room_count.saturating_add(1);
    }

    let bundle_bytes = bundle_hash.as_bytes();
    let state: String = match read_client.query_one(
        "SELECT state FROM worldstream_transfer_imports WHERE bundle_hash = $1",
        &[&bundle_bytes.as_slice()],
    ) {
        Ok(row) => match row.try_get(0) { Ok(value) => value, Err(_) => provider_failure(&mode, "runtime_read_only_state_decode_failed", evidence) },
        Err(_) => provider_failure(&mode, "runtime_read_only_state_query_failed", evidence),
    };
    let staged_chunks: i64 = match read_client.query_one(
        "SELECT count(*)::bigint FROM worldstream_transfer_chunks WHERE bundle_hash = $1",
        &[&bundle_bytes.as_slice()],
    ) {
        Ok(row) => match row.try_get(0) { Ok(value) => value, Err(_) => provider_failure(&mode, "runtime_read_only_chunk_count_decode_failed", evidence) },
        Err(_) => provider_failure(&mode, "runtime_read_only_chunk_count_query_failed", evidence),
    };
    if state != "authoritative" || staged_chunks != chunk_count as i64 {
        provider_failure(&mode, "runtime_read_only_authority_witness_mismatch", evidence);
    }
    let guard_parity = read_client
        .query_one(
            "SELECT \
             (SELECT count(*)::bigint FROM worldstream_semantic_receipts), \
             (SELECT count(*)::bigint FROM worldstream_operation_guards), \
             (SELECT count(*)::bigint \
              FROM worldstream_semantic_receipts AS receipt \
              FULL OUTER JOIN worldstream_operation_guards AS guard \
                ON guard.identity_bytes = receipt.identity_bytes \
              WHERE receipt.identity_bytes IS NULL \
                 OR guard.identity_bytes IS NULL \
                 OR receipt.canonical_request_hash IS DISTINCT FROM guard.request_hash \
                 OR receipt.room_id IS DISTINCT FROM guard.room_id \
                 OR receipt.receipt_bytes IS DISTINCT FROM guard.receipt_bytes)",
            &[],
        )
        .ok()
        .and_then(|row| {
            Some((
                row.try_get::<_, i64>(0).ok()?,
                row.try_get::<_, i64>(1).ok()?,
                row.try_get::<_, i64>(2).ok()?,
            ))
        });
    let (target_semantic_receipt_count, operation_guard_count, operation_guard_mismatch_count) =
        match guard_parity {
            Some(counts) => counts,
            None => provider_failure(&mode, "runtime_operation_guard_parity_query_failed", evidence),
        };
    let operation_guard_exact_parity_verified = target_semantic_receipt_count
        == i64::try_from(source_semantic_receipt_count).unwrap_or(-1)
        && operation_guard_count == target_semantic_receipt_count
        && operation_guard_mismatch_count == 0;
    if !operation_guard_exact_parity_verified {
        provider_failure(&mode, "runtime_operation_guard_parity_mismatch", evidence);
    }

    let probe_receipt_bytes = match read_client
        .query_opt(
            "SELECT receipt_bytes FROM worldstream_semantic_receipts ORDER BY identity_bytes LIMIT 1",
            &[],
        )
        .ok()
        .flatten()
        .and_then(|row| row.try_get::<_, Vec<u8>>(0).ok())
    {
        Some(bytes) => bytes,
        None => provider_failure(&mode, "runtime_operation_guard_probe_missing", evidence),
    };
    let probe_receipt = match StoredSemanticResultV1::from_canonical_receipt_bytes(
        &probe_receipt_bytes,
    ) {
        Ok(receipt) => receipt,
        Err(_) => provider_failure(&mode, "runtime_operation_guard_probe_corrupt", evidence),
    };
    let same_hash_stored_resolution = matches!(
        store.read_guarded_receipt(
            probe_receipt.operation_identity(),
            probe_receipt.canonical_request_hash(),
        ),
        Ok(ResolveOutcomeV1::StoredResolution(stored)) if stored.as_ref() == &probe_receipt
    );
    let mut conflicting_hash_bytes = *probe_receipt.canonical_request_hash().as_bytes();
    conflicting_hash_bytes[0] ^= 0xff;
    let conflicting_hash_hex = conflicting_hash_bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let conflicting_request_hash = match format!("blake3:{conflicting_hash_hex}")
        .parse::<CanonicalRequestHashV1>()
    {
        Ok(hash) => hash,
        Err(_) => provider_failure(&mode, "runtime_operation_guard_conflict_probe_invalid", evidence),
    };
    let different_hash_conflict = matches!(
        store.read_guarded_receipt(probe_receipt.operation_identity(), &conflicting_request_hash),
        Ok(ResolveOutcomeV1::Conflict { existing_request_hash })
            if &existing_request_hash == probe_receipt.canonical_request_hash()
    );
    if !same_hash_stored_resolution || !different_hash_conflict {
        provider_failure(&mode, "runtime_operation_guard_resolution_mismatch", evidence);
    }
    let identity_metadata_present: bool = read_client
        .query_opt(
            "SELECT 1 FROM worldstream_deployment_identity_metadata WHERE target_id = true",
            &[],
        )
        .map_err(|_| ())
        .ok()
        .flatten()
        .is_some();
    let identity_pack_count: i64 = read_client
        .query_one(
            "SELECT count(*)::bigint FROM worldstream_deployment_pack_identities",
            &[],
        )
        .map_err(|_| ())
        .ok()
        .and_then(|row| row.try_get(0).ok())
        .unwrap_or(-1);
    let identity_resource_count: i64 = read_client
        .query_one(
            "SELECT count(*)::bigint FROM worldstream_deployment_resource_identities",
            &[],
        )
        .map_err(|_| ())
        .ok()
        .and_then(|row| row.try_get(0).ok())
        .unwrap_or(-1);
    let target_identity_witness = identity_metadata_present
        && identity_pack_count == source_identity_pack_count as i64
        && identity_resource_count == source_identity_resource_count as i64;
    let target_membership_count: i64 = read_client
        .query_one("SELECT count(*)::bigint FROM worldstream_members", &[])
        .map_err(|_| ())
        .ok()
        .and_then(|row| row.try_get(0).ok())
        .unwrap_or(-1);
    let target_membership_witness = source_membership_witness
        && target_membership_count == source_membership_row_count as i64;
    let resource_blob_count: i64 = read_client
        .query_one(
            "SELECT count(*)::bigint FROM worldstream_deployment_resource_blobs",
            &[],
        )
        .ok()
        .and_then(|row| row.try_get(0).ok())
        .unwrap_or(-1);
    let resource_bytes_verified = resource_blob_count == source_evidence.resource_payloads.len() as i64
        && source_evidence.resource_payloads.iter().all(|payload| {
            let kind = match payload.identity().kind() {
                ResourceKindV1::Artifact => "artifact",
                ResourceKindV1::Codec => "codec",
                ResourceKindV1::Schema => "schema",
            };
            read_client
                .query_opt(
                    "SELECT resource_bytes, resource_digest FROM worldstream_deployment_resource_blobs WHERE resource_kind = $1 AND resource_identity = $2",
                    &[&kind, &payload.identity().identity()],
                )
                .ok()
                .flatten()
                .is_some_and(|row| {
                    row.try_get::<_, Vec<u8>>(0).ok().as_deref() == Some(payload.bytes())
                        && row.try_get::<_, Vec<u8>>(1).ok().as_deref()
                            == Some(payload.identity().digest().as_bytes().as_slice())
                })
        });
    let authority_domains = [
        ("principals", "worldstream_authority_principals"),
        ("runners", "worldstream_authority_runners"),
        ("capabilities", "worldstream_authority_capabilities"),
        ("capability_scopes", "worldstream_authority_capability_scopes"),
        (
            "runner_capability_memberships",
            "worldstream_authority_runner_capability_memberships",
        ),
        (
            "authority_change_receipts",
            "worldstream_authority_change_receipts",
        ),
        ("authority_audit", "worldstream_authority_audit"),
    ];
    let global_authority_verified = authority_domains.iter().all(|(source_table, target_table)| {
        let source_count = rows.tables.get(*source_table).map_or(0, Vec::len);
        if source_count == 0 {
            return false;
        }
        read_client
            .query_one(&format!("SELECT count(*)::bigint FROM {target_table}"), &[])
            .ok()
            .and_then(|row| row.try_get::<_, i64>(0).ok())
            == i64::try_from(source_count).ok()
    });
    let fired_receipts = read_client
        .query(
            "SELECT receipt_bytes FROM worldstream_semantic_receipts WHERE operation_kind = 'timer_fired' ORDER BY identity_bytes",
            &[],
        )
        .ok()
        .and_then(|rows| {
            rows.into_iter()
                .map(|row| {
                    let bytes = row.try_get::<_, Vec<u8>>(0).map_err(|_| ())?;
                    let stored = StoredSemanticResultV1::from_canonical_receipt_bytes(&bytes)
                        .map_err(|_| ())?;
                    let OperationIdentityV1::TimerFired(identity) = stored.operation_identity()
                    else {
                        return Err(());
                    };
                    if stored.transition_seq().is_none() {
                        return Err(());
                    }
                    Ok((
                        identity.room_id.to_string(),
                        identity.timer_id.to_string(),
                        identity.generation.get(),
                        identity.scheduled_for.as_str().to_owned(),
                    ))
                })
                .collect::<Result<BTreeSet<_>, _>>()
                .ok()
        })
        .unwrap_or_default();
    let fired_timers = read_client
        .query(
            "SELECT room_id, timer_id, generation, scheduled_for FROM worldstream_timers WHERE state = 'fired' ORDER BY room_id, timer_id, generation",
            &[],
        )
        .ok()
        .and_then(|rows| {
            rows.into_iter()
                .map(|row| {
                    Ok::<_, ()>((
                        row.try_get::<_, String>(0).map_err(|_| ())?,
                        row.try_get::<_, String>(1).map_err(|_| ())?,
                        u64::try_from(row.try_get::<_, i64>(2).map_err(|_| ())?)
                            .map_err(|_| ())?,
                        row.try_get::<_, String>(3).map_err(|_| ())?,
                    ))
                })
                .collect::<Result<BTreeSet<_>, _>>()
                .ok()
        })
        .unwrap_or_default();
    let fired_timer_linkage_verified = !fired_timers.is_empty() && fired_timers == fired_receipts;
    let isolated_integrity_incident_count: i64 = read_client
        .query_one(
            "SELECT count(*)::bigint FROM worldstream_integrity_incidents i JOIN worldstream_room_roots r ON r.room_id = i.room_id WHERE r.integrity_status <> 'healthy'",
            &[],
        )
        .ok()
        .and_then(|row| row.try_get(0).ok())
        .unwrap_or(-1);
    let generalized_path_verified = target_room_count >= 3
        && source_evidence.isolated_rooms.len() == 1
        && verified_room_count == target_room_count
        && identity_pack_count >= 2
        && resource_bytes_verified
        && global_authority_verified
        && fired_timer_linkage_verified
        && isolated_integrity_incident_count > 0;
    if !target_identity_witness {
        provider_failure(&mode, "runtime_read_only_identity_witness_mismatch", evidence);
    }
    if !target_membership_witness {
        provider_failure(&mode, "runtime_read_only_membership_witness_mismatch", evidence);
    }
    if built_generalized_fixture && !generalized_path_verified {
        provider_failure(&mode, "runtime_generalized_deployment_witness_mismatch", evidence);
    }
    if store.verify_schema().is_err() {
        provider_failure(&mode, "runtime_post_transfer_schema_verification_failed", evidence);
    }
    drop(read_client);

    evidence["postgres"] = json!({
        "required": "17.11",
        "status": "schema_verified",
        "admin_migration": "pass",
        "admin_read_only_verification": "pass",
        "runtime_read_only_verification": "pass",
        "runtime_ddl_not_used": true
    });
    evidence["transfer"] = json!({
        "status": "pass",
        "scope": "whole_deployment",
        "bundle_hash": bundle_hash.to_string(),
        "target_epoch": target.storage_epoch(),
        "record_count": bundle.records().len(),
        "operational_row_count": operational_row_count,
        "source_semantic_receipt_count": source_semantic_receipt_count,
        "target_semantic_receipt_count": target_semantic_receipt_count,
        "operation_guard_count": operation_guard_count,
        "operation_guard_mismatch_count": operation_guard_mismatch_count,
        "operation_guard_exact_parity_verified": operation_guard_exact_parity_verified,
        "post_cutover_operation_guard_resolution": {
            "same_hash": if same_hash_stored_resolution { "stored_resolution" } else { "failed" },
            "different_hash": if different_hash_conflict { "conflict" } else { "failed" }
        },
        "chunk_count": chunk_count,
        "first_chunk": first_disposition,
        "interruption_first_chunk": interruption_disposition,
        "interruption_resume_same_fenced_target": "pass",
        "conflicting_chunk_rejected": conflicting_chunk_rejected,
        "exact_preseed_rejected": exact_preseed_rejected,
        "conflicting_preseed_rejected": conflicting_preseed_rejected,
        "preseed_rejection_rolled_back": exact_preseed_rollback_clean && conflicting_preseed_rollback_clean,
        "checkpoint_resume_replay": replay_disposition,
        "pre_authority_verification": "pass",
        "target_fence": "verified",
        "read_only_final_state": state,
        "read_only_staged_chunk_count": staged_chunks,
        "full_room_semantics_hydrated": true,
        "target_room_count": target_room_count,
        "verified_room_count": verified_room_count,
        "target_room_readback": "exact_bytes_and_hashes_verified",
        "finalization": "pass",
        "target_authority": "pass",
        "authoritative_deployment_identity_gate": "pass",
        "backup_readiness_separate": true,
        "backup_readiness_required_for_transfer": false,
        "backup_diagnostic_cannot_mask_transfer_witnesses": true,
        "source_identity_witness": if source_identity_witness { "pass" } else { "fail" },
        "source_membership_witness": if source_membership_witness { "pass" } else { "fail" },
        "target_identity_witness": if target_identity_witness { "pass" } else { "fail" },
        "target_membership_witness": if target_membership_witness { "pass" } else { "fail" },
        "target_epoch_fence": "verified",
        "abort_probe": {
            "target_database_distinct": true,
            "provider_destination": "postgres",
            "provider_chunk_staged": true,
            "coordinated_abort": "pass",
            "source_authority_restored": abort_restored_authority,
            "restart_retry_idempotent": abort_idempotence_verified,
            "provider_tombstone_verified": abort_provider_tombstone_verified,
            "target_disposition": "discard_required"
        },
        "source_transfer_point": {
            "backup_verified": transfer_point.backup_digest().is_some(),
            "native_restore_ready": source_report.canonical_ready,
            "pending_restart_verified": transfer_pending_restart_verified,
            "abort_path_pending_restart_verified": pending_restart_verified,
            "abort_restored_authority": abort_restored_authority,
            "provider_derived_bundle_and_target_proof": true,
            "retired_before_target_authority": source_retired,
            "retirement_restart_verified": source_retirement_restart_verified,
            "retirement_irreversible": source_retirement_restart_verified,
            "finalization_restart_idempotent": finalization_restart_idempotence_verified
        },
        "whole_deployment_acceptance": "pass",
        "global_pack_resource_evidence": if generalized_path_verified { "pass" } else { "not_covered_by_supplied_source" },
        "general_deployment_support_verified": generalized_path_verified,
        "identity_metadata": identity_metadata_present,
        "identity_pack_count": identity_pack_count,
        "identity_resource_count": identity_resource_count,
        "resource_blob_count": resource_blob_count,
        "resource_bytes_verified": resource_bytes_verified,
        "global_authority_verified": global_authority_verified,
        "fired_timer_count": fired_timers.len(),
        "fired_timer_receipt_linkage_verified": fired_timer_linkage_verified,
        "isolated_integrity_incident_count": isolated_integrity_incident_count,
        "target_membership_count": target_membership_count,
        "isolated_room_evidence": if source_evidence.isolated_rooms.is_empty() { "none_observed" } else { "raw_bytes_preserved_without_semantic_replay" }
    });
    evidence["cleanup"] = json!({"status": "target_retained_as_authoritative_import"});
    evidence["limitations"] = json!([
        "This probe proves only the bounded source and target transfer contract for the supplied DSNs.",
        "It does not prove cross-platform packaging, signatures, SBOM/provenance, or long-duration resilience.",
        "The JSON remains operational evidence and is never release evidence."
    ]);
    evidence["acceptance"] = json!({
        "canonical_export_mechanics": "pass",
        "whole_deployment": "pass",
        "source_read_only": "pass",
        "target_migrations": "pass",
        "identity_witness": "pass",
        "authoritative_deployment_identity": "pass",
        "backup_diagnostic_separation": "pass",
        "membership_witness": "pass",
        "room_head_and_canonical_bytes": "pass",
        "operational_ledgers": "pass",
        "operation_guard_retry_semantics": "pass",
        "resource_payload_bytes": if resource_bytes_verified { "pass" } else { "not_covered" },
        "global_authority": if global_authority_verified { "pass" } else { "not_covered" },
        "fired_timer_linkage": if fired_timer_linkage_verified { "pass" } else { "not_covered" },
        "mixed_healthy_isolated_rooms": if generalized_path_verified { "pass" } else { "not_covered" },
        "pre_authority_verifier": "pass",
        "rollback_and_conflict_guards": "pass",
        "provider_derived_authority_coordinator": "pass",
        "separate_abort_target": "pass",
        "restart_replay": "pass",
        "source_transfer_lifecycle": "pass",
        "empty_target_preflight": "pass"
    });
    evidence["status"] = json!("pass");
    evidence["release_evidence"] = json!(false);
    emit(evidence, 0);
}
RS

helper_stdout="$temp_root/helper.stdout"
helper_stderr="$temp_root/helper.stderr"
helper_env=(
  "WORLDSTREAM_PG_TRANSFER_PROVIDER_MODE=$provider_mode"
  "WORLDSTREAM_PG_TRANSFER_SQLITE=$source_file"
  "WORLDSTREAM_PG_TRANSFER_ABORT_BACKUP=$source_abort_backup"
  "WORLDSTREAM_PG_TRANSFER_BACKUP=$source_transfer_backup"
  "WORLDSTREAM_PG_TRANSFER_BUILD_SOURCE=$build_source"
  "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN_FILE=$admin_dsn_file"
  "WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN_FILE=$runtime_dsn_file"
  "WORLDSTREAM_PG_TRANSFER_ABORT_ADMIN_DSN_FILE=$abort_admin_dsn_file"
  "WORLDSTREAM_PG_TRANSFER_RUNTIME_ROLE=$runtime_role"
)

set +e
env "${helper_env[@]}" CARGO_TARGET_DIR="$temp_root/cargo-target" \
  "$python_bin" - "$cargo_bin" "$helper_manifest" "$helper_stdout" "$helper_stderr" "$helper_timeout" <<'PY'
import subprocess
import sys

cargo, manifest, stdout_path, stderr_path, timeout = sys.argv[1:]
command = [cargo, "run", "--quiet", "--manifest-path", manifest, "--offline"]
try:
    with open(stdout_path, "w", encoding="utf-8") as stdout, open(
        stderr_path, "w", encoding="utf-8"
    ) as stderr:
        completed = subprocess.run(
            command,
            check=False,
            env=None,
            stdout=stdout,
            stderr=stderr,
            timeout=float(timeout),
        )
except subprocess.TimeoutExpired:
    with open(stderr_path, "a", encoding="utf-8") as stderr:
        stderr.write("transfer_driver_timeout\n")
    raise SystemExit(124)
raise SystemExit(completed.returncode)
PY
helper_code=$?
set -e

if [[ "${WORLDSTREAM_PG_TRANSFER_DEBUG:-0}" == "1" ]]; then
  sed -n '1,240p' "$helper_stderr" >&2 || true
fi

if [[ "$helper_code" -eq 124 ]]; then
  status="incomplete"
  reason="transfer_driver_timeout"
  EXIT_CODE="$EXIT_INCOMPLETE" write_static_evidence "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
elif [[ "$helper_code" -eq "$EXIT_INCOMPLETE" || "$helper_code" -eq "$EXIT_PROVIDER" || "$helper_code" -eq "$EXIT_WRONG_VERSION" || "$helper_code" -eq "$EXIT_PASS" ]]; then
  helper_json="$(tail -n 1 "$helper_stdout" 2>/dev/null || true)"
elif [[ "$helper_code" -ne 0 ]]; then
  status="incomplete"
  reason="transfer_driver_failed"
  EXIT_CODE="$EXIT_INCOMPLETE" write_static_evidence "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
else
  status="incomplete"
  reason="unexpected_transfer_driver_result"
  EXIT_CODE="$EXIT_INCOMPLETE" write_static_evidence "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi

if [[ "$helper_code" -eq "$EXIT_PASS" ]]; then
  status="pass"
fi

if [[ -z "$helper_json" ]] || ! "$python_bin" - \
  "$helper_json" "$evidence_file" "$helper_code" \
  "$POSTGRES_IMAGE" "$POSTGRES_REPOSITORY_DIGEST" "$build_source" \
  "$source_revision" <<'PY'
import json
import sys
from pathlib import Path

raw = sys.argv[1]
helper_code = int(sys.argv[3])
for forbidden in ("password=", "postgresql://", "postgres://", "ADMIN_SECRET", "RUNTIME_SECRET"):
    if forbidden in raw:
        raise SystemExit("unredacted transfer evidence")
value = json.loads(raw)
if value.get("schema") != "worldstream/sqlite-postgresql-transfer-evidence/v1":
    raise SystemExit("unexpected transfer evidence schema")
if value.get("secrets_emitted") is not False or value.get("release_evidence") is not False:
    raise SystemExit("unsafe transfer evidence flags")
status = value.get("status")
if status not in {"incomplete", "wrong_version", "pass", "unavailable"}:
    raise SystemExit("unknown transfer evidence status")
if (helper_code == 0) != (status == "pass"):
    raise SystemExit("transfer evidence status/exit mismatch")
provider_mode = value.get("provider_mode")
if provider_mode == "docker":
    value["provider_image"] = {
        "reference": sys.argv[4],
        "repository_digest": sys.argv[5],
    }
elif provider_mode == "external":
    value["provider_image"] = None
else:
    raise SystemExit("transfer evidence provider mode is not exact")
if status == "pass":
    transfer = value.get("transfer")
    source = value.get("source")
    if not isinstance(transfer, dict) or not isinstance(source, dict):
        raise SystemExit("pass evidence is missing source/transfer details")
    required_transfer = {
        "finalization": "pass",
        "target_authority": "pass",
        "full_room_semantics_hydrated": True,
        "target_room_readback": "exact_bytes_and_hashes_verified",
    }
    if any(transfer.get(key) != expected for key, expected in required_transfer.items()):
        raise SystemExit("pass evidence is missing complete target verification")
    guard_resolution = transfer.get("post_cutover_operation_guard_resolution")
    if (
        transfer.get("operation_guard_exact_parity_verified") is not True
        or transfer.get("source_semantic_receipt_count", 0) <= 0
        or transfer.get("target_semantic_receipt_count")
        != transfer.get("source_semantic_receipt_count")
        or transfer.get("operation_guard_count")
        != transfer.get("source_semantic_receipt_count")
        or transfer.get("operation_guard_mismatch_count") != 0
        or not isinstance(guard_resolution, dict)
        or guard_resolution.get("same_hash") != "stored_resolution"
        or guard_resolution.get("different_hash") != "conflict"
    ):
        raise SystemExit("pass evidence is missing exact operation-guard retry semantics")
    if transfer.get("authoritative_deployment_identity_gate") != "pass":
        raise SystemExit("pass evidence is missing authoritative deployment identity gate")
    for key in (
        "backup_readiness_separate",
        "backup_diagnostic_cannot_mask_transfer_witnesses",
    ):
        if transfer.get(key) is not True:
            raise SystemExit("pass evidence is missing backup diagnostic separation")
    if transfer.get("backup_readiness_required_for_transfer") is not False:
        raise SystemExit("native restore readiness became transfer-authoritative")
    lifecycle = transfer.get("source_transfer_point")
    if not isinstance(lifecycle, dict) or any(
        lifecycle.get(key) is not True
        for key in (
            "backup_verified",
            "pending_restart_verified",
            "abort_path_pending_restart_verified",
            "abort_restored_authority",
            "provider_derived_bundle_and_target_proof",
            "retired_before_target_authority",
            "retirement_restart_verified",
            "retirement_irreversible",
            "finalization_restart_idempotent",
        )
    ):
        raise SystemExit("pass evidence is missing the durable SQLite transfer lifecycle")
    abort_probe = transfer.get("abort_probe")
    required_abort_probe = {
        "target_database_distinct": True,
        "provider_destination": "postgres",
        "provider_chunk_staged": True,
        "coordinated_abort": "pass",
        "source_authority_restored": True,
        "restart_retry_idempotent": True,
        "provider_tombstone_verified": True,
        "target_disposition": "discard_required",
    }
    if not isinstance(abort_probe, dict) or any(
        abort_probe.get(key) != expected
        for key, expected in required_abort_probe.items()
    ):
        raise SystemExit("pass evidence is missing the isolated provider abort proof")
    for key in (
        "exact_preseed_rejected",
        "conflicting_preseed_rejected",
        "preseed_rejection_rolled_back",
    ):
        if transfer.get(key) is not True:
            raise SystemExit("pass evidence is missing atomic empty-target preflight")
    for key in (
        "source_identity_witness",
        "source_membership_witness",
        "target_identity_witness",
        "target_membership_witness",
    ):
        if transfer.get(key) != "pass":
            raise SystemExit("pass evidence is missing identity/membership witness")
    canonical = source.get("canonical_evidence")
    if not isinstance(canonical, dict) or canonical.get("status") != "complete":
        raise SystemExit("pass evidence is missing complete source verification")
    gate = source.get("transfer_contract_gate")
    backup = source.get("backup_diagnostic")
    if not isinstance(gate, dict) or gate.get("name") != "authoritative_deployment_identity_v1":
        raise SystemExit("pass evidence is missing authoritative source identity gate")
    if not isinstance(backup, dict):
        raise SystemExit("pass evidence is missing independent backup diagnostic")
    if backup.get("authoritative_for_transfer") is not False or backup.get("required_for_transfer") is not False:
        raise SystemExit("backup diagnostic did not preserve its required non-authority role")
    if sys.argv[6] == "1":
        generalized = {
            "general_deployment_support_verified": True,
            "resource_bytes_verified": True,
            "global_authority_verified": True,
            "fired_timer_receipt_linkage_verified": True,
            "isolated_room_evidence": "raw_bytes_preserved_without_semantic_replay",
        }
        if any(transfer.get(key) != expected for key, expected in generalized.items()):
            raise SystemExit("generalized transfer evidence is incomplete")
        if transfer.get("identity_pack_count", 0) < 2:
            raise SystemExit("generalized transfer evidence lacks multiple Packs")
        if transfer.get("identity_resource_count", 0) < 1 or transfer.get("resource_blob_count", 0) < 1:
            raise SystemExit("generalized transfer evidence lacks exact resource bytes")
        if transfer.get("fired_timer_count", 0) < 1:
            raise SystemExit("generalized transfer evidence lacks a fired Timer")
        if transfer.get("isolated_integrity_incident_count", 0) < 1:
            raise SystemExit("generalized transfer evidence lacks isolated incident durability")
        acceptance = value.get("acceptance")
        for key in (
            "resource_payload_bytes",
            "global_authority",
            "fired_timer_linkage",
            "mixed_healthy_isolated_rooms",
        ):
            if not isinstance(acceptance, dict) or acceptance.get(key) != "pass":
                raise SystemExit("generalized transfer acceptance is incomplete")
if sys.argv[7]:
    value["source_construction"] = {
        "classification": "untrusted_source_bound_input_construction",
        "source_revision": sys.argv[7],
        "trusted_product_execution": False,
    }
encoded = json.dumps(value, sort_keys=True, separators=(",", ":"))
if len(sys.argv) > 2 and sys.argv[2]:
    destination = Path(sys.argv[2])
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(encoded + "\n", encoding="utf-8")
print(encoded)
PY
then
  status="incomplete"
  reason="transfer_evidence_invalid_or_unredacted"
  EXIT_CODE="$EXIT_INCOMPLETE" write_static_evidence "$EXIT_INCOMPLETE"
  exit "$EXIT_INCOMPLETE"
fi

exit "$helper_code"

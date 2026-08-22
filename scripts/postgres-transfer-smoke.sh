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
admin_dsn="${WORLDSTREAM_PG_TRANSFER_ADMIN_DSN:-}"
runtime_dsn="${WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN:-}"
docker_bin="${WORLDSTREAM_PG_TRANSFER_DOCKER:-}"
cargo_bin="${WORLDSTREAM_PG_TRANSFER_CARGO:-}"
python_bin="${WORLDSTREAM_PG_TRANSFER_PYTHON:-}"
helper_timeout="${WORLDSTREAM_PG_TRANSFER_TIMEOUT_SECONDS:-180}"

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
    "Default mode creates a disposable postgres:17.11-alpine target with" \
    "separate admin/runtime credentials. Set" \
    "WORLDSTREAM_PG_TRANSFER_MODE=external together with" \
    "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN and WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN" \
    "to use an explicitly supplied, already-isolated target." \
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

if [[ -z "$python_bin" ]]; then
  if command -v python3 >/dev/null 2>&1; then
    python_bin="$(command -v python3)"
  elif command -v python >/dev/null 2>&1; then
    python_bin="$(command -v python)"
  fi
fi

if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then
  printf '%s\n' '{"schema":"worldstream/sqlite-postgresql-transfer-evidence/v1","status":"unavailable","release_evidence":false,"reason":"python3_unavailable","secrets_emitted":false}'
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

if [[ ! "$helper_timeout" =~ ^[1-9][0-9]*$ ]]; then
  status="incomplete"
  reason="invalid_helper_timeout"
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
  if [[ -z "$admin_dsn" || -z "$runtime_dsn" ]]; then
    status="incomplete"
    reason="external_target_credentials_missing"
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
      postgres:17.11-alpine >"$temp_root/docker-id" 2>"$temp_root/docker-run.log"; then
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
  admin_dsn="host=127.0.0.1 port=$port user=admin password=$admin_password dbname=worldstream"
  runtime_dsn="host=127.0.0.1 port=$port user=runtime password=$runtime_password dbname=worldstream"
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
worldstream-core = { path = "$workspace_dir/crates/worldstream-core", features = ["conformance-tracer"] }
worldstream-backup = { path = "$workspace_dir/crates/worldstream-backup" }
worldstream-postgres = { path = "$workspace_dir/crates/worldstream-postgres" }
worldstream-sqlite = { path = "$workspace_dir/crates/worldstream-sqlite" }
worldstream-transfer = { path = "$workspace_dir/crates/worldstream-transfer" }
EOF

cat >"$helper_source" <<'RS'
use std::{collections::{BTreeMap, BTreeSet}, env, path::Path};

use postgres::{Client, NoTls};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde_json::{Value, json};
use worldstream_core::{
    builtin_counter_registry, AccessModeV1, CanonicalJsonV1, CompleteHeadV1, CoreTraceV1,
    MembershipStandingV1, PackDigestV1, PackGenesisRequestV1, PackRevisionLockV1,
    ParticipantActionV1, PrincipalKindV1, RecordedStimulusV1, CORE_SCHEMA_VERSION,
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
use worldstream_sqlite::{SqliteCanonicalRecordKindV1, SqliteRoomStore};
use worldstream_transfer::{
    BackendFingerprintV1, BundleProfileV1, CanonicalRecordKindV1, DigestV1,
    NativeSqliteTransferAdapterV1, NativeSqliteTransferSpecV1, PackIdentityV1,
    SessionStatePolicyV1, TargetFingerprintV1, RecordKindV1,
    TransferChunkV1, TransferDestinationV1, TransferImportSessionV1, TransferStateV1,
};

const EXIT_INCOMPLETE: i32 = 13;
const EXIT_WRONG_VERSION: i32 = 11;
const EXIT_PROVIDER: i32 = 14;
const MAX_SOURCE_ROOMS: usize = 100_000;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;

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
    pack: Option<PackIdentityV1>,
    deployment_identity: Option<worldstream_transfer::DeploymentIdentityV1>,
    lineage_id: Option<String>,
    source_epoch: Option<u64>,
    resources: Vec<worldstream_transfer::ResourceIdentityV1>,
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
    let restore_evidence = extract_restore_evidence(source, NativeSqliteLimits::default()).ok();
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
    let connection = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| ())?;
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
    let mut pack: Option<PackIdentityV1> = None;
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
            PackIdentityV1::new(lock.pack_id, lock.revision_lock_id, digest).ok()
        });
        if !pack_lock_valid {
            missing.push(missing_evidence(
                "pack_revision_lock_unverifiable",
                "room",
                Some(&row.room_id),
                "the persisted pack revision lock is absent or does not verify against rooms.pack_digest",
            ));
        } else if let Some(candidate) = parsed_pack {
            if let Some(existing) = &pack {
                if existing != &candidate {
                    missing.push(missing_evidence(
                        "pack_identity_conflict",
                        "deployment",
                        None,
                        "Rooms do not share one persisted pack identity required by the transfer contract",
                    ));
                }
            } else {
                pack = Some(candidate);
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
        let room_complete = !isolated && pack_lock_valid && head_valid && core_valid && activity_valid && genesis_valid;
        if isolated {
            // Isolated Rooms are retained only as operational evidence. Their
            // canonical bytes must never be fed into the healthy publication
            // path, even if the bytes happen to decode successfully.
        } else if row.genesis_bytes.is_none() {
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
        if !head_valid {
            missing.push(missing_evidence(
                "room_head_bytes_unverifiable",
                "room",
                Some(&row.room_id),
                "the persisted Complete Head bytes do not verify against the Room root and lineage",
            ));
        }
        if !core_valid {
            missing.push(missing_evidence(
                "room_core_bytes_unverifiable",
                "room",
                Some(&row.room_id),
                "the persisted Core bytes are absent or do not match rooms.core_state_hash",
            ));
        }
        if !activity_valid {
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
    if let Some(export) = api_export {
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
        if export.deployment_identity().packs().len() != 1 {
            missing.push(missing_evidence(
                "deployment_pack_identity_not_projectable",
                "deployment",
                None,
                "the v0.1 transfer header requires exactly one primary pack identity",
            ));
        } else if pack.as_ref() != export.deployment_identity().packs().first() {
            missing.push(missing_evidence(
                "deployment_pack_identity_mismatch",
                "deployment",
                None,
                "the authoritative deployment identity disagrees with Room pack locks",
            ));
        } else {
            pack = export.deployment_identity().packs().first().cloned();
        }
        if !isolated_rooms.is_empty() {
            missing.push(missing_evidence(
                "isolated_rooms_present",
                "room",
                None,
                "the PostgreSQL publication seam cannot prove isolated Room preservation without canonical promotion",
            ));
        }
        let raw_room_records = canonical_records[deployment_record_count..]
            .iter()
            .filter(|record| record.kind() != RecordKindV1::Canonical(CanonicalRecordKindV1::ArtifactMetadata))
            .collect::<Vec<_>>();
        if raw_room_records.len() != export.records().len()
            || raw_room_records
                .iter()
                .zip(export.records())
                .any(|(raw, api)| raw.identity() != api.identity() || raw.bytes() != api.bytes())
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
                    SqliteCanonicalRecordKindV1::RoomHead => CanonicalRecordKindV1::RoomHead,
                    SqliteCanonicalRecordKindV1::CoreMaterialization => {
                        CanonicalRecordKindV1::CoreMaterialization
                    }
                    SqliteCanonicalRecordKindV1::ActivityMaterialization => {
                        CanonicalRecordKindV1::ActivityMaterialization
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
            for (room_id, room) in &canonical_rooms {
                let identity = format!("room/{room_id}/pack-revision-lock");
                canonical_records.push(
                    worldstream_transfer::LogicalRecordV1::canonical(
                        canonical_records.len() as u64,
                        CanonicalRecordKindV1::ArtifactMetadata,
                        &identity,
                        &room.pack_revision_lock_bytes,
                    )
                    .map_err(|_| ())?,
                );
            }
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
            if isolated_rooms.contains(&room_id) {
                if records.iter().any(|record| record.room_seq > 0) {
                    missing.push(missing_evidence(
                        "isolated_transition_evidence_not_publishable",
                        "room",
                        Some(&room_id),
                        "isolated Room Transitions are retained in source evidence but the target seam cannot publish them safely",
                    ));
                }
                continue;
            }
            let Some(room) = canonical_rooms.get_mut(&room_id) else {
                continue;
            };
            for record in records.into_iter().filter(|record| record.room_seq > 0) {
                let identity = format!("room/{room_id}/transition/{}", record.room_seq);
                canonical_records.push(
                    worldstream_transfer::LogicalRecordV1::canonical(
                        canonical_records.len() as u64,
                        CanonicalRecordKindV1::RoomTransition,
                        &identity,
                        &record.bytes,
                    )
                    .map_err(|_| ())?,
                );
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
    let expected_canonical_count = canonical_rooms
        .len()
        .saturating_mul(5)
        .saturating_add(transition_count)
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
        pack,
        deployment_identity: deployment_identity.clone(),
        lineage_id,
        source_epoch,
        resources: deployment_identity
            .as_ref()
            .map(|identity| identity.resources().to_vec())
            .unwrap_or_default(),
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

fn build_disposable_source(path: &Path) -> Result<(), ()> {
    let store = SqliteRoomStore::open(path).map_err(|_| ())?;
    store
        .initialize_canonical_metadata("deployment/live-transfer", 7)
        .map_err(|_| ())?;

    let room_id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let member_id = "01ARZ3NDEKTSV4RRFFQ69G5FC0";
    let principal_id = "01ARZ3NDEKTSV4RRFFQ69G5FD0";
    let registry = builtin_counter_registry().map_err(|_| ())?;
    let membership = worldstream_core::MembershipV1::new(
        member_id.parse().map_err(|_| ())?,
        principal_id.parse().map_err(|_| ())?,
        PrincipalKindV1::Human,
        MembershipStandingV1::Enabled,
        AccessModeV1::Participant,
        Some("counter".to_owned()),
    )
    .map_err(|_| ())?;
    let request = PackGenesisRequestV1 {
        room_id: room_id.parse().map_err(|_| ())?,
        pack_digest: worldstream_core::counter_v2_digest(),
        configuration: CanonicalJsonV1::parse(br#"{"initial_value":0,"maximum_value":4}"#)
            .map_err(|_| ())?,
        room_seed: "hex:000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
            .parse()
            .map_err(|_| ())?,
        created_at: "2026-08-21T12:00:00Z".parse().map_err(|_| ())?,
        initial_core_state:
            worldstream_core::CoreRoomStateV1::active([membership]).map_err(|_| ())?,
    };
    let prepared = registry.prepare_genesis_for_new_room(&request).map_err(|_| ())?;
    let mut trace = CoreTraceV1::create_from_retained_for_conformance(prepared).map_err(|_| ())?;
    let action_definition = trace
        .retained_pack()
        .ok_or(())?
        .descriptor()
        .actions
        .iter()
        .find(|action| action.action_type == "increment")
        .ok_or(())?;
    let action = RecordedStimulusV1::ParticipantAction(ParticipantActionV1 {
        member_id: member_id.parse().map_err(|_| ())?,
        action_id: "01ARZ3NDEKTSV4RRFFQ69G5FE0".parse().map_err(|_| ())?,
        action_type: "increment".to_owned(),
        payload_schema_digest: action_definition.payload_schema.schema_digest.clone(),
        canonical_payload: CanonicalJsonV1::parse(br"{}").map_err(|_| ())?,
        exact_basis_head: trace.head().clone(),
        admitted_at: "2026-08-21T12:00:01Z".parse().map_err(|_| ())?,
    });
    trace.advance_for_conformance(action).map_err(|_| ())?;

    let head = trace.head();
    let transition = trace.transitions().first().ok_or(())?;
    let head_bytes = head.canonical_bytes().map_err(|_| ())?;
    let genesis_bytes = trace.genesis_bytes().map_err(|_| ())?;
    let transition_bytes = transition.canonical_bytes().map_err(|_| ())?;
    let core_bytes = trace.core_state().canonical_bytes().map_err(|_| ())?;
    let activity_bytes = trace.activity_state().to_bytes().map_err(|_| ())?;
    let pack_lock_bytes = trace
        .retained_pack()
        .ok_or(())?
        .revision_lock()
        .canonical_bytes()
        .map_err(|_| ())?;
    let pack_lock = PackRevisionLockV1::from_canonical_bytes(
        &pack_lock_bytes,
        &head.pack_digest(),
    )
    .map_err(|_| ())?;
    let deployment_identity = worldstream_transfer::DeploymentIdentityV1::new(
        vec![PackIdentityV1::new(
            pack_lock.pack_id,
            pack_lock.revision_lock_id,
            DigestV1::from_bytes(head.pack_digest().digest().as_bytes()).map_err(|_| ())?,
        )
        .map_err(|_| ())?],
        Vec::new(),
    )
    .map_err(|_| ())?;
    store
        .initialize_deployment_identity(deployment_identity)
        .map_err(|_| ())?;
    drop(store);
    let member = trace
        .core_state()
        .memberships()
        .values()
        .next()
        .ok_or(())?;
    let membership_json = serde_json::to_vec(member).map_err(|_| ())?;
    let membership_bytes = CanonicalJsonV1::parse(&membership_json)
        .and_then(|value| value.to_bytes())
        .map_err(|_| ())?;
    let room_seq = i64::try_from(head.room_seq().get()).map_err(|_| ())?;
    let transition_seq = i64::try_from(transition.room_seq().get()).map_err(|_| ())?;

    let connection = Connection::open(path).map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO rooms(room_id, room_status, room_seq, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes) VALUES (?1, 'active', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                room_id,
                room_seq,
                head.genesis_or_transition_hash().to_string(),
                head.core_schema_version(),
                head.pack_digest().to_string(),
                head.core_state_hash().to_string(),
                head.activity_state_hash().to_string(),
                head.authoritative_state_hash().to_string(),
                head_bytes,
            ],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO room_genesis(room_id, pack_revision_lock_bytes, genesis_bytes) VALUES (?1, ?2, ?3)",
            rusqlite::params![room_id, pack_lock_bytes, genesis_bytes],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO room_materializations(room_id, core_state_bytes, activity_state_bytes) VALUES (?1, ?2, ?3)",
            rusqlite::params![room_id, core_bytes, activity_bytes],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO room_members(room_id, member_id, principal_id, principal_kind, standing, access_mode, role, membership_bytes, membership_generation, frame_head) VALUES (?1, ?2, ?3, 'human', 'enabled', 'participant', 'counter', ?4, 1, 0)",
            rusqlite::params![room_id, member_id, principal_id, membership_bytes],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO room_integrity(room_id, status, generation) VALUES (?1, 'healthy', 1)",
            [room_id],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO transitions(room_id, transition_id, room_seq, transition_hash, previous_lineage_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, transition_bytes) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                room_id,
                format!("transition-{transition_seq}"),
                transition_seq,
                transition.transition_hash().to_string(),
                transition.previous_lineage_hash().to_string(),
                head.core_schema_version(),
                head.pack_digest().to_string(),
                head.core_state_hash().to_string(),
                head.activity_state_hash().to_string(),
                head.authoritative_state_hash().to_string(),
                transition_bytes,
            ],
        )
        .map_err(|_| ())?;
    connection
        .execute(
            "INSERT INTO room_snapshots(room_id, room_seq, snapshot_schema_version, genesis_or_transition_hash, core_schema_version, pack_digest, core_state_hash, activity_state_hash, authoritative_state_hash, complete_head_bytes, core_state_bytes, activity_state_bytes) VALUES (?1, ?2, 'worldstream/paired-snapshot/v1', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                room_id,
                room_seq,
                head.genesis_or_transition_hash().to_string(),
                head.core_schema_version(),
                head.pack_digest().to_string(),
                head.core_state_hash().to_string(),
                head.activity_state_hash().to_string(),
                head.authoritative_state_hash().to_string(),
                head_bytes,
                core_bytes,
                activity_bytes,
            ],
        )
        .map_err(|_| ())?;
    Ok(())
}

fn main() {
    let mode = env::var("WORLDSTREAM_PG_TRANSFER_PROVIDER_MODE").unwrap_or_else(|_| "external".to_owned());
    let source = env::var("WORLDSTREAM_PG_TRANSFER_SQLITE").unwrap_or_default();
    let admin_dsn = env::var("WORLDSTREAM_PG_TRANSFER_ADMIN_DSN").unwrap_or_default();
    let runtime_dsn = env::var("WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN").unwrap_or_default();
    let mut evidence = base(&mode);

    if env::var("WORLDSTREAM_PG_TRANSFER_BUILD_SOURCE").as_deref() == Ok("1")
        && build_disposable_source(Path::new(&source)).is_err()
    {
        incomplete(&mode, "disposable_sqlite_source_build_failed", evidence);
    }

    let source_report = match verify_file(Path::new(&source), NativeSqliteLimits::default()) {
        Ok(report) => report,
        Err(_) => incomplete(&mode, "sqlite_native_verification_failed", evidence),
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

    let source_evidence = match extract_source_evidence(Path::new(&source), &source_report) {
        Ok(value) => value,
        Err(_) => incomplete(&mode, "sqlite_canonical_evidence_extraction_failed", evidence),
    };
    // The transfer contract is gated by the authenticated SQLite canonical
    // export plus its authoritative DeploymentIdentityV1 record. The native
    // backup/restore readiness diagnostic is a separate IMO-52/61 contract;
    // it is intentionally visible, but it is never transfer authority.
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
            "bytes_verified": source_evidence.deployment_identity.is_some(),
        },
        "room_pack_lock_identity_observed": source_evidence.pack.is_some(),
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

    let mut version_client = match Client::connect(&admin_dsn, NoTls) {
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

    let rows = match extract_operational_rows(Path::new(&source), NativeSqliteLimits::default()) {
        Ok(rows) => rows,
        Err(_) => incomplete(&mode, "sqlite_operational_extraction_failed", evidence),
    };
    let operational_row_count: usize = rows.tables.values().map(Vec::len).sum();
    let source_membership_row_count = rows.tables.get("room_members").map_or(0, Vec::len);
    let source_identity_pack_count = source_evidence
        .deployment_identity
        .as_ref()
        .map_or(0, |identity| identity.packs().len());
    let source_identity_resource_count = source_evidence
        .deployment_identity
        .as_ref()
        .map_or(0, |identity| identity.resources().len());
    let source_identity_witness = source_evidence.deployment_identity.is_some()
        && source_evidence.pack.is_some()
        && source_identity_pack_count == 1;
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
    let spec = NativeSqliteTransferSpecV1::new_with_deployment_identity(
        "live-whole-deployment-transfer",
        lineage_id,
        source_epoch,
        source_backend,
        target_backend,
        deployment_identity,
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

    let admin_config = match PostgresConnectionConfig::direct_admin(admin_dsn) {
        Ok(config) => config,
        Err(_) => provider_failure(&mode, "admin_dsn_rejected", evidence),
    };
    let admin = match PostgresAdmin::new(admin_config) {
        Ok(admin) => admin,
        Err(_) => provider_failure(&mode, "admin_profile_rejected", evidence),
    };
    if admin.migrate().is_err() {
        provider_failure(&mode, "postgres_admin_migration_failed", evidence);
    }
    if admin.verify_schema().is_err() {
        provider_failure(&mode, "postgres_admin_read_only_verification_failed", evidence);
    }
    let runtime_config = match PostgresConnectionConfig::runtime(
        runtime_dsn,
        PostgresConnectionPath::Direct,
    ) {
        Ok(config) => config,
        Err(_) => provider_failure(&mode, "runtime_dsn_rejected", evidence),
    };
    let store = match PostgresRoomStore::new(runtime_config) {
        Ok(store) => store,
        Err(_) => provider_failure(&mode, "runtime_profile_rejected", evidence),
    };
    if store.verify_schema().is_err() {
        provider_failure(&mode, "runtime_read_only_schema_verification_failed", evidence);
    }
    let mut destination = match PostgresTransferDestination::new(&store, &bundle, target.clone()) {
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
    let mut interrupted_destination = match PostgresTransferDestination::new(&store, &bundle, target.clone()) {
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
    if interrupted.abort(&mut interrupted_destination).is_err() {
        provider_failure(&mode, "postgres_interruption_abort_failed", evidence);
    }
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
    if let Err(error) = resumed.finalize(&mut destination, &bundle) {
        if env::var("WORLDSTREAM_PG_TRANSFER_DEBUG").as_deref() == Ok("1") {
            eprintln!("pre_authority_verification_error: {error:?}");
        }
        provider_failure(&mode, "postgres_pre_authority_verification_failed", evidence);
    }
    if let Err(error) = resumed.accept_target_write(&mut destination) {
        if env::var("WORLDSTREAM_PG_TRANSFER_DEBUG").as_deref() == Ok("1") {
            eprintln!("target_authority_commit_error: {error:?}");
        }
        provider_failure(&mode, "postgres_target_authority_commit_failed", evidence);
    }

    // Read the published Core-owned rows back through the provider's public
    // least-privileged verifier. Every healthy Room's Genesis, Head,
    // materializations, exact pack lock, Membership, operational ledgers, and
    // Transition sequence must round-trip after the authority transition.
    let target_room_count = source_evidence.canonical_rooms.len();
    let mut verified_room_count = 0usize;
    for room_id in source_evidence.canonical_rooms.keys() {
        let verification = match store.verify_room(room_id) {
            Ok(value) => value,
            Err(_) => provider_failure(&mode, "postgres_room_readback_failed", evidence),
        };
        let Some(source_room) = source_evidence.canonical_rooms.get(room_id) else {
            provider_failure(&mode, "source_room_readback_index_failed", evidence);
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
    let mut read_client = match Client::connect(
        &env::var("WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN").unwrap_or_default(),
        NoTls,
    ) {
        Ok(client) => client,
        Err(_) => provider_failure(&mode, "runtime_read_only_connection_failed", evidence),
    };
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
    if !target_identity_witness {
        provider_failure(&mode, "runtime_read_only_identity_witness_mismatch", evidence);
    }
    if !target_membership_witness {
        provider_failure(&mode, "runtime_read_only_membership_witness_mismatch", evidence);
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
        "chunk_count": chunk_count,
        "first_chunk": first_disposition,
        "interruption_first_chunk": interruption_disposition,
        "interruption_abort": "pass",
        "conflicting_chunk_rejected": conflicting_chunk_rejected,
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
        "whole_deployment_acceptance": "pass",
        "global_pack_resource_evidence": "pass",
        "identity_metadata": identity_metadata_present,
        "identity_pack_count": identity_pack_count,
        "identity_resource_count": identity_resource_count,
        "empty_resource_set_witness": identity_resource_count == 0,
        "target_membership_count": target_membership_count,
        "isolated_room_evidence": if source_evidence.isolated_rooms.is_empty() { "none_observed" } else { "preserved_as_isolated_source_evidence" }
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
        "pre_authority_verifier": "pass",
        "rollback_and_conflict_guards": "pass",
        "restart_replay": "pass"
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
  "WORLDSTREAM_PG_TRANSFER_BUILD_SOURCE=$build_source"
  "WORLDSTREAM_PG_TRANSFER_ADMIN_DSN=$admin_dsn"
  "WORLDSTREAM_PG_TRANSFER_RUNTIME_DSN=$runtime_dsn"
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

if [[ -z "$helper_json" ]] || ! "$python_bin" - "$helper_json" "$evidence_file" "$helper_code" <<'PY'
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
    if transfer.get("authoritative_deployment_identity_gate") != "pass":
        raise SystemExit("pass evidence is missing authoritative deployment identity gate")
    for key in (
        "backup_readiness_separate",
        "backup_diagnostic_cannot_mask_transfer_witnesses",
    ):
        if transfer.get(key) is not True:
            raise SystemExit("pass evidence is missing backup diagnostic separation")
    if transfer.get("backup_readiness_required_for_transfer") is not False:
        raise SystemExit("backup readiness was incorrectly made transfer-authoritative")
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
        raise SystemExit("backup diagnostic incorrectly masks transfer authority")
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

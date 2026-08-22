#!/usr/bin/env bash
set -euo pipefail

# Bounded, read-only evidence runner for the bundled SQLite native restore.
# This runner owns only an isolated temporary target. It never contacts a
# runner, publishes frames, evaluates policy, or changes pack/manifest state.

readonly SCHEMA="worldstream/native-sqlite-restore-evidence/v1"
readonly EXIT_PASS=0
readonly EXIT_UNAVAILABLE=10
readonly EXIT_CONFIGURATION=12
readonly EXIT_INCOMPLETE=13
readonly EXIT_PROVIDER=14

root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source_file="${WORLDSTREAM_NATIVE_RESTORE_SOURCE:-}"
evidence_file="${WORLDSTREAM_NATIVE_RESTORE_EVIDENCE_FILE:-}"
metadata_file="${WORLDSTREAM_NATIVE_RESTORE_METADATA_FILE:-}"
envelope_file="${WORLDSTREAM_NATIVE_RESTORE_ENVELOPE_FILE:-}"
cargo_bin="${WORLDSTREAM_NATIVE_RESTORE_CARGO:-}"
python_bin="${WORLDSTREAM_NATIVE_RESTORE_PYTHON:-}"
temp_root=""

usage() {
  printf '%s\n' \
    "Usage: scripts/native-restore-smoke.sh --source PATH --envelope PATH [--evidence PATH]" \
    "       [--metadata PATH]" \
    "Runs the bundled SQLite native restore into an isolated temporary target." \
    "--envelope is a digest-sealed NativeSqliteBackupEnvelopeV1 companion;" \
    "--metadata is retained only as a legacy boundary check; release_evidence is always false."
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --source)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native restore smoke: --source requires a path' >&2; exit "$EXIT_CONFIGURATION"; }
      source_file="$2"
      shift 2
      ;;
    --evidence)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native restore smoke: --evidence requires a path' >&2; exit "$EXIT_CONFIGURATION"; }
      evidence_file="$2"
      shift 2
      ;;
    --metadata)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native restore smoke: --metadata requires a path' >&2; exit "$EXIT_CONFIGURATION"; }
      metadata_file="$2"
      shift 2
      ;;
    --envelope)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'native restore smoke: --envelope requires a path' >&2; exit "$EXIT_CONFIGURATION"; }
      envelope_file="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit "$EXIT_PASS"
      ;;
    *)
      printf '%s\n' 'native restore smoke: unknown argument' >&2
      exit "$EXIT_CONFIGURATION"
      ;;
  esac
done

if [[ -z "$python_bin" ]]; then
  python_bin="$(command -v python3 2>/dev/null || true)"
fi
if [[ -z "$python_bin" || ! -x "$python_bin" ]]; then
  printf '%s\n' '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"unavailable","reason":"python3_unavailable","release_evidence":false,"secrets_emitted":false}'
  exit "$EXIT_UNAVAILABLE"
fi

write_result() {
  local result="$1"
  if [[ -n "$evidence_file" ]]; then
    "$python_bin" - "$evidence_file" "$result" <<'PY'
import sys
from pathlib import Path
destination = Path(sys.argv[1])
destination.parent.mkdir(parents=True, exist_ok=True)
destination.write_text(sys.argv[2] + "\n", encoding="utf-8")
PY
  fi
  printf '%s\n' "$result"
}

if [[ -z "$source_file" ]]; then
  write_result '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"incomplete","reason":"source_not_supplied","release_evidence":false,"secrets_emitted":false}'
  exit "$EXIT_INCOMPLETE"
fi
if [[ -z "$envelope_file" ]]; then
  write_result '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"incomplete","reason":"companion_envelope_not_supplied","release_evidence":false,"restore_ready":false,"secrets_emitted":false}'
  exit "$EXIT_INCOMPLETE"
fi
if [[ -L "$source_file" || ! -f "$source_file" ]]; then
  write_result '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"incomplete","reason":"source_not_a_regular_file","release_evidence":false,"secrets_emitted":false}'
  exit "$EXIT_INCOMPLETE"
fi
if [[ -L "$envelope_file" || ! -f "$envelope_file" ]]; then
  write_result '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"incomplete","reason":"companion_envelope_not_a_regular_file","release_evidence":false,"restore_ready":false,"secrets_emitted":false}'
  exit "$EXIT_INCOMPLETE"
fi

if [[ -z "$cargo_bin" ]]; then
  cargo_bin="$(command -v cargo 2>/dev/null || true)"
fi
if [[ -z "$cargo_bin" || ! -x "$cargo_bin" ]]; then
  write_result '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"unavailable","reason":"cargo_unavailable","release_evidence":false,"secrets_emitted":false}'
  exit "$EXIT_UNAVAILABLE"
fi

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-native-restore.XXXXXX")"
trap 'if [[ -n "$temp_root" && -d "$temp_root" ]]; then rm -rf "$temp_root"; fi' EXIT
driver_root="$temp_root/driver"
mkdir -p "$driver_root/src"

cat > "$driver_root/Cargo.toml" <<EOF
[package]
name = "worldstream-native-restore-smoke-driver"
version = "0.1.0"
edition = "2021"

[dependencies]
worldstream-backup = { path = "$root_dir/crates/worldstream-backup" }
worldstream-core = { path = "$root_dir/crates/worldstream-core", features = ["conformance-tracer"] }
serde_json = "=1.0.151"
EOF

cat > "$driver_root/src/main.rs" <<'RS'
use std::env;
use std::fs;
use std::path::Path;
use worldstream_backup::native_sqlite::{extract_restore_evidence, restore_file, verify_file, NativeSqliteLimits};
use worldstream_backup::{verify_native_restore, NativeSqliteBackupEnvelopeV1, VerifierLimits};
use worldstream_core::{builtin_worldstream_registry, CoreTraceV1};

fn json_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn result(status: &str, reason: &str, restore_ready: bool, extra: &str) -> String {
    let prefix = format!(
        "{{\"schema\":\"worldstream/native-sqlite-restore-evidence/v1\",\"status\":{},\"reason\":{},\"release_evidence\":false,\"restore_ready\":{},\"secrets_emitted\":false,\"side_effects\":{{\"pack\":false,\"runner_contact\":false,\"frame_publication\":false,\"policy_evaluation\":false,\"timers\":false}}",
        json_string(status), json_string(reason), restore_ready
    );
    if extra.is_empty() {
        format!("{prefix}}}")
    } else {
        format!("{prefix},{extra}}}")
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        println!("{}", result("incomplete", "driver_configuration_invalid", false, ""));
        std::process::exit(12);
    }
    let source = Path::new(&args[1]);
    let target = Path::new(&args[2]);
    let envelope_path = Path::new(&args[3]);
    let transfer = match restore_file(source, target) {
        Ok(report) => report,
        Err(_) => {
            println!("{}", result("incomplete", "native_restore_or_source_verification_failed", false, ""));
            std::process::exit(13);
        }
    };
    let verification = match verify_file(target, NativeSqliteLimits::default()) {
        Ok(report) => report,
        Err(_) => {
            println!("{}", result("incomplete", "restored_target_verification_failed", false, ""));
            std::process::exit(13);
        }
    };
    if !verification.canonical_ready {
        println!("{}", result("incomplete", "restored_target_not_canonical_ready", false, ""));
        std::process::exit(13);
    }
    let source_evidence = match extract_restore_evidence(source, NativeSqliteLimits::default()) {
        Ok(value) => value,
        Err(_) => {
            println!("{}", result("incomplete", "source_evidence_extraction_failed", false, ""));
            std::process::exit(13);
        }
    };
    let evidence = match extract_restore_evidence(target, NativeSqliteLimits::default()) {
        Ok(value) => value,
        Err(_) => {
            println!("{}", result("incomplete", "restored_evidence_extraction_failed", false, ""));
            std::process::exit(13);
        }
    };
    let registry = match builtin_worldstream_registry() {
        Ok(value) => value,
        Err(_) => {
            println!("{}", result("incomplete", "native_pack_registry_unavailable", false, ""));
            std::process::exit(13);
        }
    };
    for (room_id, records) in &evidence.canonical_records {
        let healthy = evidence
            .integrity
            .get(room_id)
            .is_some_and(|(status, _)| status == "healthy");
        if !healthy {
            continue;
        }
        let Some(genesis) = records.first() else {
            println!("{}", result("incomplete", "native_room_replay_failed", false, ""));
            std::process::exit(13);
        };
        let transitions = records
            .iter()
            .skip(1)
            .map(|record| record.bytes.clone())
            .collect::<Vec<_>>();
        if CoreTraceV1::replay(&registry, &genesis.bytes, &transitions).is_err() {
            println!("{}", result("incomplete", "native_room_replay_failed", false, ""));
            std::process::exit(13);
        }
    }
    let envelope = match fs::read(envelope_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<NativeSqliteBackupEnvelopeV1>(&bytes).ok()) {
        Some(value) => value,
        None => {
            println!("{}", result("incomplete", "companion_envelope_invalid", false, ""));
            std::process::exit(13);
        }
    };
    let native_evidence = match envelope.build_native_restore_evidence(
        &source_evidence,
        &evidence,
        transfer.capture_witness(),
        VerifierLimits::default(),
    ) {
        Ok(value) => value,
        Err(error) => {
            let reason = match error {
                worldstream_backup::NativeSqliteEnvelopeError::EnvelopeDigestMismatch => "companion_envelope_digest_mismatch",
                worldstream_backup::NativeSqliteEnvelopeError::NativeEvidenceMismatch => "native_restore_evidence_mismatch",
                worldstream_backup::NativeSqliteEnvelopeError::GlobalDigestMismatch => "native_restore_global_digest_mismatch",
                worldstream_backup::NativeSqliteEnvelopeError::IncompleteCompanion => "native_restore_companion_incomplete",
                worldstream_backup::NativeSqliteEnvelopeError::CaptureWitnessMismatch => "native_restore_capture_witness_mismatch",
                _ => "native_restore_companion_invalid",
            };
            println!("{}", result("incomplete", reason, false, ""));
            std::process::exit(13);
        }
    };
    let report = verify_native_restore(&native_evidence, VerifierLimits::default());
    let ready = report.is_ready();
    let reason = if ready { "native_restore_verified" } else { "native_restore_verifier_blocked" };
    let extra = format!(
        "\"restore\":{{\"isolated_target\":true,\"page_count\":{},\"source_canonical_ready\":{},\"destination_canonical_ready\":{}}},\"verification\":{{\"engine_version\":{},\"query_only\":{},\"integrity_check\":{},\"canonical_ready\":true,\"room_count\":{},\"snapshot_count\":{},\"diagnostic_count\":{}}},\"semantic_verifier\":{{\"invoked\":true,\"ready\":{},\"release_evidence\":false}}",
        transfer.page_count,
        transfer.source_canonical_ready,
        transfer.destination_canonical_ready,
        json_string(&verification.engine_version),
        verification.query_only,
        json_string(&verification.integrity_check),
        verification.room_count,
        verification.snapshot_count,
        report.diagnostics.len(),
        ready,
    );
    println!("{}", result(if ready { "ready" } else { "incomplete" }, reason, ready, &extra));
    std::process::exit(if ready { 0 } else { 13 });
}
RS

target_file="$temp_root/isolated-target.sqlite"
set +e
if [[ -n "$metadata_file" ]]; then
  if [[ -L "$metadata_file" || ! -f "$metadata_file" ]]; then
    write_result '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"incomplete","reason":"metadata_adapter_not_a_regular_file","release_evidence":false,"secrets_emitted":false}'
    exit "$EXIT_INCOMPLETE"
  fi
  driver_output="$(WORLDSTREAM_NATIVE_RESTORE_METADATA_FILE="$metadata_file" "$cargo_bin" run --quiet --manifest-path "$driver_root/Cargo.toml" --offline -- "$source_file" "$target_file" "$envelope_file" 2>/dev/null)"
else
  driver_output="$($cargo_bin run --quiet --manifest-path "$driver_root/Cargo.toml" --offline -- "$source_file" "$target_file" "$envelope_file" 2>/dev/null)"
fi
driver_code=$?
set -e
if [[ -z "$driver_output" ]]; then
  write_result '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"unavailable","reason":"bundled_driver_unavailable","release_evidence":false,"secrets_emitted":false}'
  exit "$EXIT_UNAVAILABLE"
fi
last_line="$(printf '%s\n' "$driver_output" | tail -n 1)"
if [[ "$driver_code" -eq 0 ]]; then
  write_result "$last_line"
  exit "$EXIT_PASS"
fi
if [[ "$driver_code" -eq 13 ]]; then
  write_result "$last_line"
  exit "$EXIT_INCOMPLETE"
fi
write_result '{"schema":"worldstream/native-sqlite-restore-evidence/v1","status":"unavailable","reason":"bundled_driver_failed","release_evidence":false,"secrets_emitted":false}'
exit "$EXIT_PROVIDER"

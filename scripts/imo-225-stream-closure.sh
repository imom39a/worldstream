#!/usr/bin/env bash
set -euo pipefail

# Local, non-release IMO-225 SQLite-to-PostgreSQL v2 closure evidence.
readonly POSTGRES_IMAGE="postgres:17.11-alpine@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly POSTGRES_REPOSITORY_DIGEST="postgres@sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
readonly MINIMUM_TRANSITIONS=100001

workspace_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$workspace_dir"

transitions="$MINIMUM_TRANSITIONS"
evidence_file=""
keep_artifacts=0
docker_bin="${WORLDSTREAM_IMO225_DOCKER:-docker}"
cargo_bin="${WORLDSTREAM_IMO225_CARGO:-cargo}"

usage() {
  printf '%s\n' \
    "Usage: scripts/imo-225-stream-closure.sh [--transitions N] [--evidence PATH] [--keep-artifacts]" \
    "Runs local non-release SQLite-to-PostgreSQL v2 stream closure evidence."
}

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --transitions)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'IMO-225 closure: --transitions requires a value' >&2; exit 2; }
      transitions="$2"; shift 2 ;;
    --evidence)
      [[ "$#" -ge 2 ]] || { printf '%s\n' 'IMO-225 closure: --evidence requires a path' >&2; exit 2; }
      evidence_file="$2"; shift 2 ;;
    --keep-artifacts) keep_artifacts=1; shift ;;
    --help|-h) usage; exit 0 ;;
    *) printf '%s\n' 'IMO-225 closure: unknown argument' >&2; exit 2 ;;
  esac
done

if [[ ! "$transitions" =~ ^[0-9]+$ ]] || (( transitions < MINIMUM_TRANSITIONS )); then
  printf '%s\n' "IMO-225 closure: --transitions must be an integer >= ${MINIMUM_TRANSITIONS}" >&2
  exit 2
fi
if [[ -n "$evidence_file" && -e "$evidence_file" ]]; then
  printf '%s\n' 'IMO-225 closure: evidence destination must not already exist' >&2
  exit 2
fi
command -v "$docker_bin" >/dev/null 2>&1 || { printf '%s\n' 'IMO-225 closure: Docker is unavailable' >&2; exit 10; }
command -v "$cargo_bin" >/dev/null 2>&1 || { printf '%s\n' 'IMO-225 closure: Cargo is unavailable' >&2; exit 10; }
command -v openssl >/dev/null 2>&1 || { printf '%s\n' 'IMO-225 closure: OpenSSL is unavailable' >&2; exit 10; }

temp_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-imo225-closure.XXXXXX")"
container_name="worldstream-imo225-${RANDOM}-${RANDOM}"
container_started=0
cleanup() {
  local cleanup_code=0
  if [[ "$container_started" -eq 1 ]]; then
    "$docker_bin" rm -f "$container_name" >/dev/null 2>&1 || cleanup_code=1
  fi
  if [[ "$keep_artifacts" -eq 0 ]]; then
    rm -rf "$temp_root"
  else
    printf '%s\n' "IMO-225 closure artifacts retained at: $temp_root" >&2
  fi
  return "$cleanup_code"
}
trap cleanup EXIT

image_digest="$($docker_bin image inspect "$POSTGRES_IMAGE" --format '{{index .RepoDigests 0}}' 2>/dev/null || true)"
if [[ "$image_digest" != "$POSTGRES_REPOSITORY_DIGEST" ]]; then
  "$docker_bin" pull "$POSTGRES_IMAGE" >/dev/null
  image_digest="$($docker_bin image inspect "$POSTGRES_IMAGE" --format '{{index .RepoDigests 0}}')"
fi
[[ "$image_digest" == "$POSTGRES_REPOSITORY_DIGEST" ]] || { printf '%s\n' 'IMO-225 closure: pinned PostgreSQL image digest verification failed' >&2; exit 11; }

postgres_password="$(openssl rand -hex 24)"
"$docker_bin" run --detach --rm --name "$container_name" \
  --env POSTGRES_USER=admin \
  --env POSTGRES_PASSWORD="$postgres_password" \
  --env POSTGRES_DB=worldstream \
  --publish 127.0.0.1::5432 "$POSTGRES_IMAGE" >/dev/null
container_started=1
for attempt in $(seq 1 60); do
  "$docker_bin" exec "$container_name" pg_isready -U admin -d worldstream >/dev/null 2>&1 && break
  sleep 1
done
"$docker_bin" exec "$container_name" pg_isready -U admin -d worldstream >/dev/null
provider_version="$($docker_bin exec "$container_name" psql -U admin -d worldstream -Atqc 'SHOW server_version')"
[[ "$provider_version" == 17.11* ]] || { printf '%s\n' 'IMO-225 closure: PostgreSQL provider is not 17.11' >&2; exit 11; }
"$docker_bin" exec "$container_name" createdb -U admin worldstream_corrupt

postgres_port="$($docker_bin port "$container_name" 5432/tcp | sed -n 's/.*:\([0-9][0-9]*\)$/\1/p')"
[[ "$postgres_port" =~ ^[0-9]+$ ]] || { printf '%s\n' 'IMO-225 closure: Docker did not expose a PostgreSQL port' >&2; exit 12; }
printf 'host=127.0.0.1 port=%s user=admin password=%s dbname=worldstream sslmode=disable\n' "$postgres_port" "$postgres_password" > "$temp_root/admin.dsn"
printf 'host=127.0.0.1 port=%s user=admin password=%s dbname=worldstream_corrupt sslmode=disable\n' "$postgres_port" "$postgres_password" > "$temp_root/corrupt-admin.dsn"
chmod 600 "$temp_root/admin.dsn" "$temp_root/corrupt-admin.dsn"

"$cargo_bin" build --locked --release -p worldstream-sqlite --example history_qualification_fixture
"$cargo_bin" build --locked --release -p worldstream-server --example stream_closure_evidence
./target/release/examples/history_qualification_fixture \
  --database "$temp_root/source.sqlite" --transition-count "$transitions" \
  --stream-metadata --output "$temp_root/source-report.json"
./target/release/examples/stream_closure_evidence \
  --source "$temp_root/source.sqlite" --backup "$temp_root/source.backup.sqlite" \
  --restored-backup "$temp_root/source.restored.sqlite" \
  --stream "$temp_root/source.stream" --corrupted-stream "$temp_root/source.corrupted.stream" \
  --output "$temp_root/report.json" --admin-dsn-file "$temp_root/admin.dsn" \
  --corrupt-admin-dsn-file "$temp_root/corrupt-admin.dsn" \
  --stream-id "imo-225-local-$(date +%s)-${RANDOM}"

if [[ -n "$evidence_file" ]]; then
  mkdir -p "$(dirname "$evidence_file")"
  install -m 600 "$temp_root/report.json" "$evidence_file"
fi
printf '%s\n' "IMO-225 closure: pass postgres=$provider_version transitions=$transitions release_evidence=false"

#!/usr/bin/env bash
set -euo pipefail

# Install the exact PostgreSQL 17.11 client bytes used by hosted native-restore
# evidence. The server is separately pinned to the 17.11 Alpine image.

if [[ -z "${RUNNER_TEMP:-}" || -z "${GITHUB_PATH:-}" || -z "${GITHUB_ENV:-}" ]]; then
  printf '%s\n' 'hosted runner paths are required for pinned PostgreSQL clients' >&2
  exit 1
fi

readonly client_name='postgresql-client-17_17.11-1.pgdg24.04+2_amd64.deb'
readonly client_url='https://apt.postgresql.org/pub/repos/apt/pool/main/p/postgresql-17/postgresql-client-17_17.11-1.pgdg24.04%2B2_amd64.deb'
readonly client_size='2052656'
readonly client_sha256='b3b071b67a814a382516d6f6241b529c52f909672457e2f48083acce047b634f'

readonly common_name='postgresql-client-common_293.pgdg24.04+1_all.deb'
readonly common_url='https://apt.postgresql.org/pub/repos/apt/pool/main/p/postgresql-common/postgresql-client-common_293.pgdg24.04%2B1_all.deb'
readonly common_size='48488'
readonly common_sha256='80ae115f63ba67fbba442f6b75a468a559b77a3ee3b0be357a4c49e70bf860c5'

readonly libpq_name='libpq5_18.6-1.pgdg24.04+2_amd64.deb'
readonly libpq_url='https://apt.postgresql.org/pub/repos/apt/pool/main/p/postgresql-18/libpq5_18.6-1.pgdg24.04%2B2_amd64.deb'
readonly libpq_size='264072'
readonly libpq_sha256='b487c5ed2ceb9244c6a9d6ae65818ed6707c3c44a2e14488394ae4194c52c53b'

readonly download_root="$RUNNER_TEMP/worldstream-postgresql-client-17.11"
test ! -e "$download_root"
install -d -m 700 "$download_root"

download_exact() {
  local name="$1"
  local url="$2"
  local size="$3"
  local digest="$4"
  local destination="$download_root/$name"

  test ! -e "$destination"
  curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 \
    "$url" --output "$destination"
  test "$(wc -c < "$destination" | tr -d '[:space:]')" = "$size"
  printf '%s  %s\n' "$digest" "$destination" | sha256sum --check --strict -
}

download_exact "$common_name" "$common_url" "$common_size" "$common_sha256"
download_exact "$libpq_name" "$libpq_url" "$libpq_size" "$libpq_sha256"
download_exact "$client_name" "$client_url" "$client_size" "$client_sha256"

sudo dpkg --install \
  "$download_root/$common_name" \
  "$download_root/$libpq_name" \
  "$download_root/$client_name"

readonly postgres_bin='/usr/lib/postgresql/17/bin'
for tool in pg_dump pg_restore psql; do
  executable="$postgres_bin/$tool"
  test -x "$executable"
  observed_version="$($executable --version)"
  case "$observed_version" in
    *' 17.11'*) ;;
    *)
      printf '%s\n' "unexpected PostgreSQL provider version: $observed_version" >&2
      exit 1
      ;;
  esac
done

printf '%s\n' "$postgres_bin" >> "$GITHUB_PATH"
printf 'WORLDSTREAM_PG_DUMP=%s\n' "$postgres_bin/pg_dump" >> "$GITHUB_ENV"
printf 'WORLDSTREAM_PG_RESTORE=%s\n' "$postgres_bin/pg_restore" >> "$GITHUB_ENV"
printf 'WORLDSTREAM_PSQL=%s\n' "$postgres_bin/psql" >> "$GITHUB_ENV"

printf '%s\n' 'Installed byte-pinned PostgreSQL 17.11 provider clients.'

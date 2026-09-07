#!/bin/sh
set -eu

volume_root="${WORLDSTREAM_HOSTED_VOLUME_ROOT:-/var/lib/worldstream}"
secret_root="${WORLDSTREAM_HOSTED_EPHEMERAL_ROOT:-/run/worldstream}/secrets"

if [ "$(id -u)" != "0" ]; then
  echo 'Hosted bootstrap must start as root and drop privileges itself.' >&2
  exit 78
fi
if [ "$volume_root" != "/var/lib/worldstream" ] || [ ! -d "$volume_root" ] || [ -L "$volume_root" ]; then
  echo 'Hosted volume contract is invalid.' >&2
  exit 78
fi
if [ ! -r /proc/self/mountinfo ] || ! awk -v path="$volume_root" '$5 == path { found = 1 } END { exit found ? 0 : 1 }' /proc/self/mountinfo; then
  echo 'Hosted volume must be an explicit mount at /var/lib/worldstream.' >&2
  exit 78
fi

: "${WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET:?missing Runtime bootstrap secret}"
: "${WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY:?missing Controller authority}"
: "${WORLDSTREAM_VERCEL_SERVICE_AUTHORITY:?missing Vercel service authority}"
: "${OPENROUTER_API_KEY:?missing OpenRouter credential}"

validate_secret_length() {
  secret_name="$1"
  secret_value="$2"
  if [ "${#secret_value}" -lt 32 ] || [ "${#secret_value}" -gt 512 ]; then
    echo "Hosted secret $secret_name has an invalid length." >&2
    exit 78
  fi
}
# The kernel reads this file as 32 raw bytes, not a hex/base64-encoded key.
# Count the bytes that printf will write rather than locale-dependent characters.
if [ "$(printf '%s' "$WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET" | wc -c)" -ne 32 ]; then
  echo 'Runtime bootstrap secret must contain exactly 32 bytes.' >&2
  exit 78
fi
validate_secret_length WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY "$WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY"
validate_secret_length WORLDSTREAM_VERCEL_SERVICE_AUTHORITY "$WORLDSTREAM_VERCEL_SERVICE_AUTHORITY"
validate_secret_length OPENROUTER_API_KEY "$OPENROUTER_API_KEY"
unset secret_value secret_name

case "${WORLDSTREAM_DEVELOPMENT_IDENTITY_BYPASS:-}${WORLDSTREAM_DEVELOPMENT_FAKE_OPENROUTER:-}" in
  "") ;;
  *)
    echo 'Development substitutes are forbidden in the hosted image.' >&2
    exit 78
    ;;
esac

umask 077
install -d -m 0700 -o 65532 -g 65532 "$secret_root"
printf '%s' "$WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET" > "$secret_root/authority-bootstrap"
printf '%s' "$WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY" > "$secret_root/controller-authority"
printf '%s' "$WORLDSTREAM_VERCEL_SERVICE_AUTHORITY" > "$secret_root/vercel-service-authority"
printf '%s' "$OPENROUTER_API_KEY" > "$secret_root/openrouter-api-key"
chown 65532:65532 "$secret_root"/* "$volume_root"
chmod 0700 "$volume_root"
chmod 0600 "$secret_root"/*

unset WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET
unset WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY
unset WORLDSTREAM_VERCEL_SERVICE_AUTHORITY
unset OPENROUTER_API_KEY

export WORLDSTREAM_AUTHORITY_BOOTSTRAP_SECRET_FILE="$secret_root/authority-bootstrap"
export WORLDSTREAM_HOSTED_CONTROLLER_AUTHORITY_FILE="$secret_root/controller-authority"
export WORLDSTREAM_VERCEL_SERVICE_AUTHORITY_FILE="$secret_root/vercel-service-authority"
export WORLDSTREAM_OPENROUTER_API_KEY_FILE="$secret_root/openrouter-api-key"

exec setpriv \
  --reuid=65532 \
  --regid=65532 \
  --clear-groups \
  --no-new-privs \
  --inh-caps=-all \
  --ambient-caps=-all \
  --bounding-set=-all \
  node /opt/worldstream/hosted/hosted-runtime.mjs

#!/usr/bin/env bash
set -euo pipefail

# IMO-58 optional collector witness. This is the only script allowed to claim
# collector delivery, and it does so only after an actual OpenTelemetry
# Collector container receives a request from the standard-library transport.

collector_image="${WORLDSTREAM_OTEL_COLLECTOR_IMAGE:-otel/opentelemetry-collector-contrib:latest}"

if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
  printf '%s\n' '{"schema":"worldstream/telemetry-collector-smoke/v1","status":"SKIP_INCOMPLETE","reason":"docker_unavailable","collector_delivery":"not_claimed","release_evidence":false}'
  exit 0
fi

if ! docker image inspect "$collector_image" >/dev/null 2>&1; then
  docker pull "$collector_image" >/dev/null
fi
collector_digest="$(docker image inspect --format '{{index .RepoDigests 0}}' "$collector_image" 2>/dev/null || true)"
if [[ -z "$collector_digest" ]]; then
  printf '%s\n' '{"schema":"worldstream/telemetry-collector-smoke/v1","status":"SKIP_INCOMPLETE","reason":"collector_image_digest_unavailable","collector_delivery":"not_claimed","release_evidence":false}'
  exit 0
fi

for command_name in cargo python3 mktemp; do
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf 'telemetry collector smoke: required command is unavailable: %s\n' "$command_name" >&2
    exit 2
  fi
done

smoke_root="$(mktemp -d "${TMPDIR:-/tmp}/worldstream-telemetry-collector.XXXXXX")"
container_name="worldstream-otel-$$"
collector_config="$smoke_root/collector.yaml"
collector_log="$smoke_root/collector.log"
cleanup() {
  docker rm -f "$container_name" >/dev/null 2>&1 || true
  rm -rf "$smoke_root"
}
trap cleanup EXIT

cat >"$collector_config" <<'YAML'
receivers:
  otlp:
    protocols:
      http:
        endpoint: 0.0.0.0:4318
exporters:
  debug:
    verbosity: detailed
service:
  pipelines:
    logs:
      receivers: [otlp]
      exporters: [debug]
YAML

collector_port="$(python3 - <<'PY'
import socket
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    print(listener.getsockname()[1])
PY
)"

docker run --rm -d --name "$container_name" \
  -p "127.0.0.1:${collector_port}:4318" \
  -v "$collector_config:/etc/otelcol-contrib/config.yaml:ro" \
  "$collector_image" --config /etc/otelcol-contrib/config.yaml >/dev/null

for _ in {1..120}; do
  docker logs "$container_name" >"$collector_log" 2>&1 || true
  if grep -q 'Everything is ready' "$collector_log"; then
    break
  fi
  if ! docker inspect -f '{{.State.Running}}' "$container_name" 2>/dev/null | grep -q true; then
    sed 's|/tmp/worldstream-telemetry-collector[^ ]*|<temp>|g' "$collector_log" >&2
    printf '%s\n' 'telemetry collector smoke: collector exited before readiness' >&2
    exit 1
  fi
  sleep 0.1
done

if ! grep -q 'Everything is ready' "$collector_log"; then
  sed 's|/tmp/worldstream-telemetry-collector[^ ]*|<temp>|g' "$collector_log" >&2
  printf '%s\n' 'telemetry collector smoke: collector did not become ready' >&2
  exit 1
fi

WORLDSTREAM_TELEMETRY_COLLECTOR_ENDPOINT="http://127.0.0.1:${collector_port}/v1/logs" \
  cargo test --locked -p worldstream-server --lib \
  telemetry::tests::standard_http_transport_exercises_configured_external_collector -- --nocapture

sleep 0.5
docker logs "$container_name" >"$collector_log" 2>&1
if ! grep -q 'worldstream/telemetry/v1' "$collector_log"; then
  sed 's|/tmp/worldstream-telemetry-collector[^ ]*|<temp>|g' "$collector_log" >&2
  printf '%s\n' 'telemetry collector smoke: collector logs contained no WorldStream telemetry record' >&2
  exit 1
fi
if grep -Eq 'Bearer|password|secret|dsn|room_id' "$collector_log"; then
  sed 's|/tmp/worldstream-telemetry-collector[^ ]*|<temp>|g' "$collector_log" >&2
  printf '%s\n' 'telemetry collector smoke: redaction marker found in collector output' >&2
  exit 1
fi

python3 - "$collector_digest" "$collector_port" <<'PY'
import json
import sys

print(json.dumps({
    "schema": "worldstream/telemetry-collector-smoke/v1",
    "status": "passed",
    "collector_delivery": "verified",
    "collector_image_digest": sys.argv[1],
    "collector_port": int(sys.argv[2]),
    "release_evidence": False,
    "secrets": "not_emitted",
}, sort_keys=True, separators=(",", ":")))
PY

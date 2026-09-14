#!/usr/bin/env bash
set -euo pipefail

readonly evidence=/data/imo-232-linux-1m.json
readonly status_file=/data/status
readonly manifest=/data/runtime-manifest.json
readonly retained_fixture_archive=/data/fixture-1000000.sqlite.gz
readonly retained_fixture_report=/data/fixture-1000000.json
readonly started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

mkdir -p /data/tmp
rm -f "${evidence}.tmp" "$status_file"

failure() {
  code="$?"
  printf 'failed:%s\n' "$code" > "$status_file"
  exit "$code"
}
trap failure ERR

/usr/bin/python3 - "$manifest" "$started_at" <<'PY'
import json
import os
import platform
import sys
from pathlib import Path

destination, started_at = sys.argv[1:]
meminfo = {}
for line in Path("/proc/meminfo").read_text().splitlines():
    key, value = line.split(":", 1)
    meminfo[key] = value.strip()
stat = os.statvfs("/data")
report = {
    "schema": "worldstream/imo-232-linux-runtime/v1",
    "release_evidence": False,
    "started_at": started_at,
    "platform": {
        "system": platform.system(),
        "machine": platform.machine(),
        "kernel": platform.release(),
        "cpu_count": os.cpu_count(),
        "memory_total": meminfo.get("MemTotal"),
    },
    "volume": {
        "block_size": stat.f_frsize,
        "total_bytes": stat.f_blocks * stat.f_frsize,
        "available_bytes_at_start": stat.f_bavail * stat.f_frsize,
    },
    "source_revision": Path("/opt/worldstream/source-revision.txt").read_text().strip(),
    "artifact_sha256": Path("/opt/worldstream/artifact-sha256.txt").read_text().splitlines(),
}
Path(destination).write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
PY

if [[ -f "$retained_fixture_archive" && -f "$retained_fixture_report" ]]; then
  reused_source=/data/tmp/reused-history-1000000.sqlite
  gzip -dc "$retained_fixture_archive" > "$reused_source"
  /opt/worldstream/imo-220-warm-claim-qualification.sh \
    --evidence "${evidence}.tmp" \
    --samples 1000 \
    --tier 1000000 \
    --source-database "$reused_source" \
    --fixture-report "$retained_fixture_report"
else
  /opt/worldstream/imo-220-warm-claim-qualification.sh \
    --evidence "${evidence}.tmp" \
    --samples 1000 \
    --tier 1000000
fi
mv "${evidence}.tmp" "$evidence"
for leaves in 1000 10000 100000 1000000; do
  /usr/local/bin/operational_mmr_qualification \
    --leaves "$leaves" \
    --samples 1000 \
    --output "/data/operational-mmr-${leaves}.json"
done
printf 'pass\n' > "$status_file"
trap - ERR
exec sleep infinity

#!/usr/bin/env bash
set -euo pipefail

readonly evidence=/data/imo-232-linux-1m.json
readonly status_file=/data/status
readonly manifest=/data/runtime-manifest.json
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

/opt/worldstream/imo-220-warm-claim-qualification.sh \
  --evidence "${evidence}.tmp" \
  --samples 1000 \
  --tier 1000000
mv "${evidence}.tmp" "$evidence"
printf 'pass\n' > "$status_file"
trap - ERR
exec sleep infinity

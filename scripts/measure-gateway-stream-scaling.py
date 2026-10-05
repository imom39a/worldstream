#!/usr/bin/env python3
"""Run isolated opt-in actual gateway test-binary cells and sample child RSS."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time
ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("scaling_metadata", ROOT / "scripts/measure-room-stream-scaling.py")
common = importlib.util.module_from_spec(spec)
spec.loader.exec_module(common)
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=Path, required=True)
parser.add_argument("--build-profile", choices=["debug","release"], default="debug")
parser.add_argument("--sessions", type=common.positive_list, default=[1,10,64])
parser.add_argument("--topology", choices=["same-membership","distinct-rooms","aggregate"], required=True)
parser.add_argument("--formats", default="v1,v2")
parser.add_argument("--pattern",choices=["paced","burst","stalled-reader"],default="paced")
parser.add_argument("--publication-mode", choices=["shared", "independent"], default="shared")
parser.add_argument("--updates", type=int, default=100)
parser.add_argument("--output-dir", type=Path, required=True)
args = parser.parse_args()
output = args.output_dir.resolve(); output.mkdir(parents=True, exist_ok=False)
report = {"schema":"worldstream/gateway-measurement-driver/v1", "metadata":common.metadata(), "cells":[]}
report["metadata"]["publication_mode"] = args.publication_mode
report["metadata"]["build_profile"] = f"Cargo test {args.build_profile}; {'optimized' if args.build_profile=='release' else 'unoptimized'}; conformance-tracer; opt-in measurement cfg"
report["metadata"]["rss_attribution"] = "Single Rust subprocess includes loopback server, SQLite embedded engine and synchronous client driver. Python and time wrapper excluded. No separate daemon RSS claimed."
report["metadata"]["binary_sha256"] = common.digest(args.binary.resolve())
report["metadata"]["harness_sha256"] = common.digest(ROOT / "crates/worldstream-server/src/sqlite_backend/tests/gateway_scaling_measurement.rs")
try:
    for fmt in args.formats.split(","):
        if fmt not in ("v1","v2"): parser.error("formats must be v1,v2")
        for sessions in args.sessions:
            label = f"{fmt}-{args.topology}-{sessions}"
            path = output / f"{label}.json"; timing = output / f"{label}.time.txt"
            env = dict(os.environ, WORLDSTREAM_GATEWAY_MEASUREMENT_PUBLICATION_MODE=args.publication_mode, WORLDSTREAM_GATEWAY_MEASUREMENT_PATTERN=args.pattern,WORLDSTREAM_GATEWAY_MEASUREMENT_FORMAT=fmt, WORLDSTREAM_GATEWAY_MEASUREMENT_TOPOLOGY=args.topology, WORLDSTREAM_GATEWAY_MEASUREMENT_SESSIONS=str(sessions), WORLDSTREAM_GATEWAY_MEASUREMENT_UPDATES=str(args.updates), WORLDSTREAM_GATEWAY_MEASUREMENT_OUTPUT=str(path))
            argv = ["/usr/bin/time", "-l" if sys.platform=="darwin" else "-v",str(args.binary.resolve()),"measure_actual_gateway_scaling","--ignored","--test-threads=1","--nocapture"]
            peak = 0; baseline = None; samples = 0; began=time.monotonic()
            with (output/f"{label}.stdout.txt").open("w") as out, timing.open("w") as err:
                child = subprocess.Popen(argv,cwd=ROOT,env=env,stdout=out,stderr=err)
                while child.poll() is None:
                    for line in common.command("ps","-axo","pid=,ppid=,rss=").splitlines():
                        fields=line.split()
                        if len(fields)==3 and int(fields[1])==child.pid:
                            rss=int(fields[2])*1024; baseline=rss if baseline is None else baseline; peak=max(peak,rss);samples+=1
                    time.sleep(.05)
                status=child.wait()
            maximum=None
            for line in timing.read_text().splitlines():
                if "maximum resident set size" in line: maximum=int(line.split()[0])
                elif "Maximum resident set size (kbytes):" in line: maximum=int(line.rsplit(":",1)[1])*1024
            cell={"publication_mode":args.publication_mode,"format":fmt,"topology":args.topology,"requested_sessions":sessions,"exit_code":status,"elapsed_seconds":time.monotonic()-began,"rust_rss_baseline_bytes":baseline,"rust_rss_sampled_peak_bytes":peak if samples else None,"rss_samples":samples,"rust_time_maximum_rss_bytes":maximum}
            if path.exists() and status==0: cell["measurement"]=json.loads(path.read_text())
            else:
                cell["status"]="failed"
                if path.exists(): cell["diagnostic"]=json.loads(path.read_text())
            report["cells"].append(cell); (output/"report.json").write_text(json.dumps(report,indent=2))
            print(f"{label}: exit={status}, elapsed={cell['elapsed_seconds']:.2f}s",flush=True)
finally:
    (output/"report.json").write_text(json.dumps(report,indent=2))

if any(cell["exit_code"] != 0 for cell in report["cells"]):
    sys.exit(1)

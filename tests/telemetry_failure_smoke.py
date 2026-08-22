#!/usr/bin/env python3
"""Boundary tests for the IMO-58 telemetry failure smoke runner."""

from __future__ import annotations

import os
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "telemetry-failure-smoke.sh"
COLLECTOR_SCRIPT = ROOT / "scripts" / "telemetry-collector-smoke.sh"


FIXTURE_DAEMON = textwrap.dedent(
    r"""
    #!/usr/bin/env python3
    import argparse
    import http.server
    import json
    import signal
    import threading

    parser = argparse.ArgumentParser()
    parser.add_argument("--data-dir", required=True)
    parser.add_argument("--bind", required=True)
    args = parser.parse_args()
    if "not-a-url-telemetry-secret-marker" in __import__("os").environ.get(
        "WORLDSTREAM__TELEMETRY__OTLP__ENDPOINT", ""
    ):
        print(
            '{"event":"storage_diagnostic","reason":"exporter_malformed_endpoint"}',
            flush=True,
        )

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def send_json(self, status, body):
            payload = json.dumps(body, separators=(",", ":")).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def do_GET(self):
            if self.path == "/healthz":
                self.send_json(200, {"status": "ok"})
            elif self.path == "/readyz":
                self.send_json(200, {"status": "ready"})
            elif self.path == "/metrics":
                self.send_response(200)
                self.send_header("Content-Type", "text/plain")
                self.end_headers()
                self.wfile.write(
                    b"worldstream_telemetry_enqueued_total 2\n"
                    b"worldstream_telemetry_exporter_failures_total 0\n"
                    b"worldstream_telemetry_dropped_total 0\n"
                    b'worldstream_telemetry_events_total{event="migration"} 2\n'
                )
            elif self.path == "/version":
                self.send_json(
                    200,
                    {
                        "manifest": {"release_ready": True},
                        "engine": {"status": "verified"},
                    },
                )
            elif self.path == "/v1/rooms/room-fixture/projection":
                self.send_json(
                    200,
                    {
                        "room_id": "room-fixture",
                        "room_head": {
                            "room_seq": 0,
                            "authoritative_state_hash": "sha256:fixture",
                        },
                    },
                )
            else:
                self.send_json(404, {"error": {"code": "not_found"}})

        def do_POST(self):
            if self.path == "/v1/operator/member-capabilities":
                self.send_json(
                    200,
                    {
                        "room_id": "room-fixture",
                        "member_id": "member-fixture",
                        "principal_id": "01ARZ3NDEKTSV4RRFFQ69G5FC2",
                        "bearer": "wsb1:" + "cd" * 32,
                    },
                )
            elif self.path == "/v1/rooms":
                if self.headers.get("Authorization", "").endswith("ab" * 32):
                    self.send_json(400, {"error": {"code": "invalid"}})
                    return
                self.send_json(
                    200,
                    {
                        "room_id": "room-fixture",
                        "member_ids": ["member-fixture"],
                        "room_head": {
                            "room_seq": 0,
                            "authoritative_state_hash": "sha256:fixture",
                        },
                    },
                )
            else:
                self.send_json(403, {"error": {"code": "forbidden"}})

    bind_host, bind_port = args.bind.rsplit(":", 1)
    server = http.server.ThreadingHTTPServer((bind_host, int(bind_port)), Handler)
    print('{"listen_address":"fixture"}', flush=True)

    def stop(*_):
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, stop)
    server.serve_forever()
    """
).lstrip()


class TelemetryFailureSmokeBoundaryTests(unittest.TestCase):
    def run_smoke(self, daemon: Path) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        fake_bin = daemon.parent / "fake-bin"
        fake_bin.mkdir()
        fake_od = fake_bin / "od"
        fake_od.write_text(
            "#!/bin/sh\n"
            "printf '%s\\n' '00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 "
            "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00'\n",
            encoding="utf-8",
        )
        fake_od.chmod(0o700)
        environment.update(
            {
                "WORLDSTREAMD_BIN": str(daemon),
                "PATH": f"{fake_bin}{os.pathsep}{environment['PATH']}",
            }
        )
        return subprocess.run(
            ["bash", str(SCRIPT)],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            timeout=45,
            check=False,
        )

    def test_fixture_covers_failure_boundary_without_release_claim(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="telemetry-failure-smoke-test-"
        ) as temporary:
            daemon = Path(temporary) / "fixture-worldstreamd"
            daemon.write_text(FIXTURE_DAEMON, encoding="utf-8")
            daemon.chmod(0o700)
            result = self.run_smoke(daemon)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("telemetry failure", result.stdout)

    def test_missing_binary_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="telemetry-failure-smoke-test-"
        ) as temporary:
            result = self.run_smoke(Path(temporary) / "missing-worldstreamd")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("daemon is not executable", result.stderr)

    def test_process_witness_does_not_claim_collector_delivery(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('"collector_delivery": "not_claimed"', source)
        self.assertIn('"release_evidence": False', source)
        self.assertIn('event="migration"', source)
        self.assertIn("SQLite migration producer did not emit", source)

    def test_collector_witness_requires_a_real_container_and_records_digest(
        self,
    ) -> None:
        source = COLLECTOR_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("docker run --rm -d", source)
        self.assertIn("collector_image_digest", source)
        self.assertIn('"collector_delivery": "verified"', source)
        self.assertIn('"release_evidence": False', source)


if __name__ == "__main__":
    unittest.main()

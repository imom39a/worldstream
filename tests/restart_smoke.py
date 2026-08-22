#!/usr/bin/env python3
"""Boundary tests for scripts/restart-smoke.sh when worldstreamd is unavailable."""

from __future__ import annotations

import os
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts" / "restart-smoke.sh"


FIXTURE_DAEMON = textwrap.dedent(
    r"""
    #!/usr/bin/env python3
    import argparse
    import http.server
    import json
    import os
    from pathlib import Path
    import signal
    import threading

    parser = argparse.ArgumentParser()
    parser.add_argument("--data-dir", required=True)
    parser.add_argument("--bind", required=True)
    args = parser.parse_args()
    data_dir = Path(args.data_dir)
    if data_dir.exists() and not data_dir.is_dir():
        raise SystemExit(17)
    data_dir.mkdir(mode=0o700, parents=True, exist_ok=True)
    (data_dir / "worldstream.sqlite3").write_bytes(b"fixture sqlite")

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
                if os.environ.get("FIXTURE_READY", "true") == "true":
                    self.send_json(200, {"status": "ready"})
                else:
                    self.send_json(503, {"error": {"code": "storage_not_initialized"}})
            elif self.path == "/version":
                self.send_json(200, {
                    "manifest": {"release_ready": True},
                    "engine": {"status": "verified", "exact_identity": "sqlite/test"},
                })
            else:
                self.send_json(404, {"error": {"code": "not_found"}})

        def do_POST(self):
            self.send_json(403, {"error": {"code": "forbidden"}})

    bind_host, bind_port = args.bind.rsplit(":", 1)
    server = http.server.ThreadingHTTPServer((bind_host, int(bind_port)), Handler)
    print(json.dumps({"listen_address": args.bind}, separators=(",", ":")), flush=True)

    def stop(*_):
        threading.Thread(target=server.shutdown, daemon=True).start()

    signal.signal(signal.SIGTERM, stop)
    server.serve_forever()
    server.server_close()
    """
).lstrip()


class RestartSmokeBoundaryTests(unittest.TestCase):
    def run_smoke(
        self, daemon: Path, **extra_env: str
    ) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        fake_bin = daemon.parent / "fake-bin"
        fake_bin.mkdir()
        fake_uname = fake_bin / "uname"
        fake_uname.write_text("#!/bin/sh\nprintf '%s\\n' Linux\n", encoding="utf-8")
        fake_uname.chmod(0o700)
        fake_stat = fake_bin / "stat"
        fake_stat.write_text(
            "#!/usr/bin/env python3\n"
            "import os, sys\n"
            "path = sys.argv[3]\n"
            "print(os.stat(path).st_size if sys.argv[2] == '%s' else os.stat(path).st_ino)\n",
            encoding="utf-8",
        )
        fake_stat.chmod(0o700)
        environment.update(
            {
                "WORLDSTREAMD_BIN": str(daemon),
                "PATH": f"{fake_bin}{os.pathsep}{environment['PATH']}",
                **extra_env,
            }
        )
        return subprocess.run(
            ["bash", str(SCRIPT)],
            cwd=ROOT,
            env=environment,
            text=True,
            capture_output=True,
            timeout=30,
            check=False,
        )

    def write_fixture(self, directory: Path) -> Path:
        daemon = directory / "fixture-worldstreamd"
        daemon.write_text(FIXTURE_DAEMON, encoding="utf-8")
        daemon.chmod(0o700)
        return daemon

    def test_fixture_covers_restart_and_preserves_fail_closed_observations(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory(prefix="restart-smoke-test-") as temporary:
            result = self.run_smoke(self.write_fixture(Path(temporary)))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("readiness observed", result.stdout)
        self.assertIn("synthetic bearer remains forbidden", result.stdout)

    def test_ready_failure_is_rejected_instead_of_claimed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="restart-smoke-test-") as temporary:
            result = self.run_smoke(
                self.write_fixture(Path(temporary)), FIXTURE_READY="false"
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected HTTP 200", result.stderr)

    def test_missing_binary_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory(prefix="restart-smoke-test-") as temporary:
            missing = Path(temporary) / "does-not-exist"
            result = self.run_smoke(missing)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("daemon is not executable", result.stderr)


if __name__ == "__main__":
    unittest.main()

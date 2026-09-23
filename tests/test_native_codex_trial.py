"""Regression checks for native trial identity selection and frozen checks."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "scripts"))
from agent_swarm_native_trial import NativeTrial


class NativeIdentity(unittest.TestCase):
    def test_requested_native_is_selected_among_multiple_codex_installations(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            native, guard = root / "codex-native", root / "guard"
            native.write_bytes(b"synthetic native identity, never executed")
            guard.write_bytes(b"synthetic guard identity, never executed")
            auth_root = root / "normal-login"
            auth_root.mkdir()
            (auth_root / "auth.json").write_text("{}")
            args = SimpleNamespace(codex=native, guard=guard, swarm_app=root / "app")
            rows = [
                {"provider": "codex", "capabilities": None},
                {
                    "provider": "codex",
                    "capabilities": {
                        "executable": str(root / "other-native"),
                        "executable_digest": "wrong",
                    },
                },
                {
                    "provider": "codex",
                    "capabilities": {
                        "executable": str(native),
                        "executable_digest": "exact-requested-identity",
                    },
                },
            ]
            observed = subprocess.CompletedProcess(
                [], 0, stdout=json.dumps(rows).encode()
            )
            with (
                patch.dict("os.environ", {"CODEX_HOME": str(auth_root)}),
                patch("agent_swarm_native_trial.subprocess.run", return_value=observed),
            ):
                trial = NativeTrial(args, root / "trial", root / "work")
            try:
                self.assertEqual(trial.executable_digest, "exact-requested-identity")
                self.assertEqual(trial.profile.stat().st_mode & 0o777, 0o700)
                self.assertEqual(
                    (trial.profile / "config.toml").stat().st_mode & 0o777, 0o600
                )
            finally:
                trial.close()
            self.assertFalse(trial.profile.exists())


if __name__ == "__main__":
    unittest.main()

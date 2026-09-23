"""Integrity boundaries for the live evaluation's model-visible check evidence."""

from __future__ import annotations

import importlib.util
import json
import socket
import subprocess
import sys
from pathlib import Path
from unittest.mock import patch

import pytest
from blake3 import blake3

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
sys.path.insert(0, str(SCRIPTS))
SPEC = importlib.util.spec_from_file_location(
    "swarm_challenge_fixture", SCRIPTS / "agent_swarm_challenge.py"
)
assert SPEC is not None and SPEC.loader is not None
CHALLENGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHALLENGE)
with patch.dict(sys.modules, {"agent_swarm_challenge": CHALLENGE}):
    from agent_swarm_autonomous_delivery import authorized_checker_command
    from agent_swarm_live import check_sandbox_command, verified_check_text


def autonomous_sandbox_command(_root, script):
    program, arguments = authorized_checker_command()
    arguments[-1] = script
    assert all(len(argument.encode()) <= 4096 for argument in arguments)
    return program, arguments


def output_object(root, data):
    digest = blake3(data).hexdigest()
    (root / "objects").mkdir()
    path = root / "objects" / digest
    path.write_bytes(data)
    return path, {"digest": "blake3:" + digest, "byte_length": len(data)}


def test_check_feedback_requires_exact_bytes(tmp_path):
    path, ref = output_object(tmp_path, b'{"failed":["round_trip"]}')
    assert verified_check_text(tmp_path, ref) == '{"failed":["round_trip"]}'
    path.write_bytes(b'{"failed":[]}')
    with pytest.raises(RuntimeError, match="identity mismatch"):
        verified_check_text(tmp_path, ref)


def test_check_feedback_rejects_path_and_symlink_substitution(tmp_path):
    with pytest.raises(RuntimeError, match="invalid check output digest"):
        verified_check_text(tmp_path, {"digest": "blake3:../../outside"})
    path, ref = output_object(tmp_path, b"output")
    target = tmp_path / "elsewhere"
    path.rename(target)
    path.symlink_to(target)
    with pytest.raises(RuntimeError, match="unsafe check output object"):
        verified_check_text(tmp_path, ref)


def test_check_feedback_is_bounded_after_full_digest_verification(tmp_path):
    _, ref = output_object(tmp_path, b"x" * 20000)
    assert verified_check_text(tmp_path, ref) == "x" * 16384 + "\n[truncated]"


@pytest.mark.skipif(sys.platform != "darwin", reason="macOS Seatbelt boundary")
@pytest.mark.parametrize(
    "launcher", [check_sandbox_command, autonomous_sandbox_command]
)
def test_native_check_sandbox_denies_writes_outside_reads_and_network(
    tmp_path, launcher
):
    root = tmp_path / "candidate"
    root.mkdir()
    (root / "allowed").write_text("candidate")
    outside = tmp_path / "outside"
    outside.write_text("canary")
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        address = listener.getsockname()
        with socket.create_connection(address, timeout=1):
            pass
        script = f"""
import errno, json, socket
from pathlib import Path
assert Path('allowed').read_text() == 'candidate'
denials = []
def denied(name, operation):
    try: operation()
    except OSError as error:
        assert error.errno in (errno.EACCES, errno.EPERM), str(error)
        denials.append(name)
    else: raise AssertionError(name + ' was allowed')
denied('outside_read', lambda: Path({str(outside)!r}).read_text())
denied('candidate_write', lambda: Path('new-file').write_text('no'))
denied('outside_write', lambda: Path({str(outside)!r}).write_text('no'))
denied('reachable_network', lambda: socket.create_connection({address!r}, timeout=1))
print(json.dumps(denials))
"""
        program, arguments = launcher(root, script)
        result = subprocess.run(
            [program, *arguments],
            cwd=root,
            env={},
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == [
        "outside_read",
        "candidate_write",
        "outside_write",
        "reachable_network",
    ]
    assert outside.read_text() == "canary"
    assert not (root / "new-file").exists()

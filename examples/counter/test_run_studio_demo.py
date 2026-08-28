from __future__ import annotations

import importlib.util
import json
import socket
import stat
import sys
import tempfile
import threading
from pathlib import Path
from types import SimpleNamespace

import pytest

path = Path(__file__).with_name("run_studio_demo.py")
sys_path = str(path.parent)
if sys_path not in sys.path:
    sys.path.insert(0, sys_path)
from deterministic_loopback_provider import create_provider_server

spec = importlib.util.spec_from_file_location("counter_studio_demo", path)
assert spec and spec.loader
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def test_existing_state_requires_an_explicit_retain_choice() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary) / "demo-state"
        root.mkdir(mode=0o700)
        with pytest.raises(module.DemoConfigurationError, match="retain-state"):
            module.prepare_state_directory(root, retain_state=False)

        prepared, ephemeral = module.prepare_state_directory(root, retain_state=True)

    assert prepared == root.resolve()
    assert ephemeral is False


def test_control_file_contains_only_loopback_urls_and_safe_fixture_metadata() -> None:
    payload = module.control_payload(
        supervisor_port=9410,
        daemon_port=9420,
        studio_port=5174,
        console_port=5173,
    )

    assert payload["schema"] == "worldstream/counter-studio-demo-control/v1"
    assert payload["urls"] == {
        "supervisor": "http://127.0.0.1:9410",
        "daemon": "http://127.0.0.1:9420",
        "studio": "http://127.0.0.1:5174",
        "console": "http://127.0.0.1:5173",
    }
    assert payload["fixture"]["credential_id"] == "local-openai"
    assert payload["fixture"]["provider_url"] == "http://127.0.0.1:19431"
    assert payload["fixture"]["model_id"] == "counter-deterministic"
    encoded = json.dumps(payload).lower()
    assert "secret_reference" not in encoded
    assert "token" not in encoded
    assert "bearer" not in encoded


def test_control_file_is_published_owner_only() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        parent = Path(temporary) / "control"
        parent.mkdir(mode=0o700)
        path = parent / "demo.json"
        module.write_control_file(path, module.control_payload(9410, 9420, 5174, 5173))

        assert stat.S_IMODE(path.stat().st_mode) == 0o600
        assert json.loads(path.read_text("utf-8"))["status"] == "ready"


def test_control_file_keeps_a_custom_public_provider_port() -> None:
    payload = module.control_payload(9410, 9420, 5174, 5173, provider_port=19_433)

    assert payload["fixture"]["provider_url"] == "http://127.0.0.1:19433"


def test_control_file_rejects_a_symlink_target() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        parent = Path(temporary) / "control"
        parent.mkdir(mode=0o700)
        target = parent / "target.json"
        target.write_text("{}", encoding="utf-8")
        target.chmod(0o600)
        link = parent / "demo.json"
        link.symlink_to(target.name)

        with pytest.raises(module.DemoConfigurationError, match="control file"):
            module.write_control_file(link, module.control_payload(9410, 9420, 5174, 5173))


def test_reused_control_file_is_marked_building_before_dependencies(monkeypatch: pytest.MonkeyPatch) -> None:
    with tempfile.TemporaryDirectory() as temporary:
        parent = Path(temporary) / "control"
        parent.mkdir(mode=0o700)
        control = parent / "demo.json"
        module.write_control_file(control, module.control_payload(9410, 9420, 5174, 5173))
        observed: list[str] = []

        def build() -> None:
            observed.append(json.loads(control.read_text("utf-8"))["status"])
            raise module.DemoConfigurationError("stop after control transition")

        monkeypatch.setattr(module, "require_ports_available", lambda _ports: None)
        monkeypatch.setattr(module, "build_dependencies", build)
        arguments = SimpleNamespace(
            supervisor_port=9410,
            daemon_port=9420,
            studio_port=5174,
            console_port=5173,
            provider_port=19431,
            skip_build=False,
            control_file=control,
        )
        with pytest.raises(module.DemoConfigurationError, match="stop after control transition"):
            module.start_demo(arguments)
        assert observed == ["building"]
        assert json.loads(control.read_text("utf-8"))["status"] == "building"


def test_port_configuration_rejects_collisions_before_starting_processes() -> None:
    with pytest.raises(module.DemoConfigurationError, match="distinct"):
        module.validate_ports(9410, 9410, 5174, 5173, 19431)


def test_port_preflight_rejects_a_live_listener_but_allows_its_immediate_reuse() -> None:
    listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    listener.bind(("127.0.0.1", 0))
    listener.listen()
    port = listener.getsockname()[1]
    with pytest.raises(module.DemoConfigurationError, match="already in use"):
        module.require_ports_available([port])
    listener.close()
    module.require_ports_available([port])


def test_vite_command_passes_loopback_and_strict_port_to_vite() -> None:
    command = module.vite_command("web/studio", 5174)

    assert command == [
        "pnpm", "--dir", "web/studio", "exec", "vite",
        "--host", "127.0.0.1", "--port", "5174", "--strictPort",
    ]
    assert "--" not in command


def test_provider_readiness_uses_the_real_bounded_fixture_status_route() -> None:
    with tempfile.TemporaryDirectory() as temporary:
        credential = Path(temporary) / "model-token"
        credential.write_bytes(b"fixture-model-token-not-for-logs")
        credential.chmod(0o600)
        server = create_provider_server("127.0.0.1", 0, credential)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            url = f"http://127.0.0.1:{server.server_address[1]}"
            assert module.provider_status_ready(url)
            assert not module.provider_status_ready(url + "/wrong-base")
        finally:
            server.shutdown()
            thread.join(timeout=2)
            server.server_close()

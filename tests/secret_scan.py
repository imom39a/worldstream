"""Exact channel tests for the release secret-absence scanner."""

from __future__ import annotations

import base64
import hashlib
import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/verify-secret-absence.py"


def load_module():
    spec = importlib.util.spec_from_file_location("worldstream_secret_scan", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_scan_binds_every_clean_channel(tmp_path):
    module = load_module()
    sentinel = b"0123456789abcdef0123456789abcdef"
    sentinel_path = tmp_path / "sentinel"
    sentinel_path.write_bytes(sentinel)
    channels = {}
    for name in ("logs", "image-config", "history", "report"):
        path = tmp_path / name
        path.write_bytes(b"" if name == "logs" else f"clean {name}\n".encode())
        channels[name] = path

    report = module.scan(sentinel_path, channels)

    assert report["status"] == "pass"
    assert report["secrets_emitted"] is False
    assert [row["channel"] for row in report["channels"]] == sorted(channels)
    assert next(row for row in report["channels"] if row["channel"] == "logs") == {
        "channel": "logs",
        "sha256": "sha256:" + "e3b0c44298fc1c149afbf4c8996fb924"
        "27ae41e4649b934ca495991b7852b855",
        "size_bytes": 0,
    }
    assert report["encodings_scanned"] == ["base64", "base64url", "hex", "raw"]


@pytest.mark.parametrize("channel", ["logs", "image-config", "history", "report"])
@pytest.mark.parametrize("encoding", ["raw", "hex", "base64", "base64url"])
def test_scan_rejects_each_encoded_secret_in_each_channel(tmp_path, channel, encoding):
    module = load_module()
    sentinel = b"0123456789abcdef0123456789abcdef"
    sentinel_path = tmp_path / "sentinel"
    sentinel_path.write_bytes(sentinel)
    encoded = {
        "raw": sentinel,
        "hex": sentinel.hex().encode(),
        "base64": base64.b64encode(sentinel),
        "base64url": base64.urlsafe_b64encode(sentinel).rstrip(b"="),
    }[encoding]
    channels = {}
    for name in ("logs", "image-config", "history", "report"):
        path = tmp_path / name
        path.write_bytes(encoded if name == channel else b"clean\n")
        channels[name] = path

    with pytest.raises(module.ScanError, match=f"channel {channel}"):
        module.scan(sentinel_path, channels)


@pytest.mark.parametrize(
    "encoded",
    [
        b"0123456789ABCDEF0123456789ABCDEF".hex().upper().encode(),
        base64.b64encode(b"0123456789ABCDEF0123456789ABCDEF").rstrip(b"="),
    ],
)
def test_scan_rejects_uppercase_hex_and_unpadded_base64(tmp_path, encoded):
    module = load_module()
    sentinel = b"0123456789ABCDEF0123456789ABCDEF"
    sentinel_path = tmp_path / "sentinel"
    sentinel_path.write_bytes(sentinel)
    channel = tmp_path / "logs"
    channel.write_bytes(encoded)

    with pytest.raises(module.ScanError, match="channel logs"):
        module.scan(sentinel_path, {"logs": channel})


def test_multi_sentinel_scan_retains_exact_channel_and_sentinel_digests(tmp_path):
    module = load_module()
    sentinels = {
        "authority-secret": b"\xfb" * 32,
        "member-capability": b"wsb1:" + b"b" * 61 + b"~~~",
        "postgres-password": b"c" * 45 + b"~~~",
        "postgres-dsn": b"host=127.0.0.1 password=" + b"d" * 45 + b"~~~",
    }
    channels = {}
    for name, content in {
        "daemon-log": b"clean daemon output\n",
        "child-stdout": b"clean child output\n",
        "child-stderr": b"",
        "child-config": b'{"dsn_source":"owner_only_file"}\n',
        "child-report": b'{"secrets":"not_emitted"}\n',
    }.items():
        path = tmp_path / name
        path.write_bytes(content)
        channels[name] = path

    report = module.scan_sentinels(sentinels, channels)

    assert report["schema"] == "worldstream/secret-absence-matrix/v1"
    assert report["status"] == "pass"
    assert report["secrets_emitted"] is False
    assert report["encodings_scanned"] == ["base64", "base64url", "hex", "raw"]
    assert [row["name"] for row in report["sentinels"]] == sorted(sentinels)
    assert [row["channel"] for row in report["channels"]] == sorted(channels)
    for row in report["sentinels"]:
        value = sentinels[row["name"]]
        assert row["sha256"] == "sha256:" + hashlib.sha256(value).hexdigest()
        assert row["size_bytes"] == len(value)


@pytest.mark.parametrize(
    "sentinel_name",
    ["authority-secret", "member-capability", "postgres-password", "postgres-dsn"],
)
@pytest.mark.parametrize(
    "channel",
    ["daemon-log", "child-stdout", "child-stderr", "child-config", "child-report"],
)
@pytest.mark.parametrize("encoding", ["raw", "hex", "base64", "base64url"])
def test_multi_sentinel_scan_rejects_exact_injection(
    tmp_path, sentinel_name, channel, encoding
):
    module = load_module()
    sentinels = {
        "authority-secret": b"\xfb" * 32,
        "member-capability": b"wsb1:" + b"b" * 61 + b"~~~",
        "postgres-password": b"c" * 45 + b"~~~",
        "postgres-dsn": b"host=127.0.0.1 password=" + b"d" * 45 + b"~~~",
    }
    encoded = module.sentinel_forms(sentinels[sentinel_name])[encoding]
    channels = {}
    for name in (
        "daemon-log",
        "child-stdout",
        "child-stderr",
        "child-config",
        "child-report",
    ):
        path = tmp_path / name
        path.write_bytes(encoded if name == channel else b"clean\n")
        channels[name] = path

    with pytest.raises(
        module.ScanError,
        match=rf"sentinel {sentinel_name} detected in channel {channel} as {encoding}",
    ):
        module.scan_sentinels(sentinels, channels)

"""Focused tests for measured reference-host disclosures."""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk/python/src"))


def load_soak():
    spec = importlib.util.spec_from_file_location(
        "worldstream_daemon_transition_soak_environment",
        ROOT / "scripts/daemon-transition-soak.py",
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def main() -> None:
    soak = load_soak()
    with tempfile.TemporaryDirectory(prefix="worldstream-host-observation-") as value:
        root = pathlib.Path(value)
        os_release = root / "os-release"
        os_release.write_text(
            'NAME="Ubuntu"\nVERSION_ID="24.04"\nID=ubuntu\n', encoding="utf-8"
        )
        assert soak.linux_distribution(os_release) == {
            "distribution": "Ubuntu",
            "distribution_version": "24.04",
        }

        mount = root / "data"
        mount.mkdir()
        mountinfo = root / "mountinfo"
        mountinfo.write_text(
            f"36 25 8:1 / {mount.resolve()} rw,relatime - ext4 /dev/nvme0n1p1 rw\n",
            encoding="utf-8",
        )
        device = root / "devices/pci/block/nvme0n1/nvme0n1p1"
        (device.parent / "queue").mkdir(parents=True)
        (device.parent / "queue/rotational").write_text("0\n", encoding="utf-8")
        device.mkdir()
        sys_dev_block = root / "sys-dev-block"
        sys_dev_block.mkdir()
        (sys_dev_block / "8:1").symlink_to(device)
        assert soak.filesystem_environment(mount, mountinfo, sys_dev_block) == {
            "type": "ext4",
            "mount_options": ["relatime", "rw"],
            "storage_class": "local_ssd_or_nvme",
        }

        (device.parent / "queue/rotational").write_text("1\n", encoding="utf-8")
        try:
            soak.filesystem_environment(mount, mountinfo, sys_dev_block)
        except soak.SoakFailure as error:
            assert "local SSD/NVMe" in str(error)
        else:
            raise AssertionError("rotational storage was accepted")

        os_release.write_text('NAME="Ubuntu\nVERSION_ID="24.04"\n', encoding="utf-8")
        try:
            soak.linux_distribution(os_release)
        except soak.SoakFailure as error:
            assert "malformed" in str(error)
        else:
            raise AssertionError("malformed os-release input was accepted")

    print("daemon transition soak host observation: PASS")


if __name__ == "__main__":
    main()

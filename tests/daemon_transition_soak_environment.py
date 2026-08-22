"""Focused tests for measured reference-host disclosures."""

from __future__ import annotations

import copy
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
    binary_sha256 = "sha256:" + "1" * 64
    disk_full = {
        "schema": soak.DISK_FULL_SCHEMA,
        "status": "pass",
        "release_evidence": False,
        "scenario": soak.DISK_FULL_SCENARIO,
        "evidence_class": soak.DISK_FULL_EVIDENCE_CLASS,
        "platform": {
            "system": "Linux",
            "machine": "x86_64",
            "filesystem": "ext4",
        },
        "container": {
            "image": soak.DISK_FULL_CONTAINER_IMAGE,
            "platform": "linux/amd64",
            "privileged": True,
            "network": "none",
            "root_filesystem_read_only": True,
            "daemon_mount_read_only": True,
            "observed_elapsed_ms": 500.0,
            "captured_output_bytes": 1024,
        },
        "bounds": {
            "filesystem_image_bytes": soak.DISK_FULL_FILESYSTEM_IMAGE_BYTES,
            "attempted_write_bytes": soak.DISK_FULL_ATTEMPTED_WRITE_BYTES,
            "max_daemon_seconds": soak.DISK_FULL_MAX_DAEMON_SECONDS,
            "max_container_seconds": soak.DISK_FULL_MAX_CONTAINER_SECONDS,
            "max_log_bytes": soak.DISK_FULL_MAX_LOG_BYTES,
            "max_capture_bytes": soak.DISK_FULL_MAX_CAPTURE_BYTES,
        },
        "filesystem": {
            "type": "ext4",
            "mount_source_class": "loop_device",
            "image_size_bytes": soak.DISK_FULL_FILESYSTEM_IMAGE_BYTES,
            "block_size_bytes": soak.DISK_FULL_BLOCK_SIZE_BYTES,
            "available_kib_after_fill": 0,
            "database_file_type": "regular",
            "database_file_mode": "0600",
            "fill_bytes_written": 55_988_224,
        },
        "fault": {
            "errno_number": 28,
            "errno_name": "ENOSPC",
            "attempted_write_bytes": soak.DISK_FULL_ATTEMPTED_WRITE_BYTES,
            "write_returned_bytes": 0,
            "database_size_before_bytes": 0,
            "database_size_after_bytes": 0,
        },
        "daemon": {
            "binary_sha256": binary_sha256,
            "started": True,
            "exit_observed": True,
            "exit_code": 1,
            "ready_http_200_observed": False,
            "public_mutation_available": False,
            "storage_failure_observed": True,
            "elapsed_ms": 100.0,
            "diagnostic_bytes": 128,
            "diagnostic_sha256": "sha256:" + "2" * 64,
        },
        "cleanup": {
            "internal_unmount_observed": True,
            "container_remove_requested": True,
            "container_absent_after_run": True,
        },
        "limitations": {
            "runtime_disk_exhaustion_recovery_observed": False,
            "physical_power_loss_observed": False,
        },
    }
    assert (
        soak.validate_disk_full_evidence(disk_full, binary_sha256=binary_sha256)
        == disk_full
    )
    for field, value in (
        (("fault", "errno_name"), "EIO"),
        (("filesystem", "available_kib_after_fill"), 1),
        (("container", "image"), "docker@sha256:" + "0" * 64),
        (("daemon", "ready_http_200_observed"), True),
        (("daemon", "binary_sha256"), "sha256:" + "3" * 64),
        (("cleanup", "container_absent_after_run"), False),
    ):
        malformed = copy.deepcopy(disk_full)
        malformed[field[0]][field[1]] = value
        try:
            soak.validate_disk_full_evidence(malformed, binary_sha256=binary_sha256)
        except soak.SoakFailure as error:
            assert "disk-full evidence was incomplete" in str(error)
        else:
            raise AssertionError(f"malformed disk-full field {field} was accepted")

    marker_values = {
        "filesystem_type": "ext4",
        "mount_source_class": "loop_device",
        "image_size_bytes": "67108864",
        "block_size_bytes": "4096",
        "available_kib_after_fill": "0",
        "database_file_type": "regular",
        "database_file_mode": "0600",
        "fill_bytes_written": "55988224",
        "fault_errno_name": "ENOSPC",
        "fault_attempted_write_bytes": "4096",
        "fault_write_returned_bytes": "0",
        "database_size_before_bytes": "0",
        "database_size_after_bytes": "0",
        "daemon_exit_code": "1",
        "daemon_ready_http_200_observed": "0",
        "daemon_storage_failure_observed": "1",
        "daemon_uptime_start_seconds": "10.00",
        "daemon_uptime_end_seconds": "10.15",
        "diagnostic_bytes": "128",
        "diagnostic_sha256": "2" * 64,
        "internal_unmount_observed": "1",
    }

    def marker_output(values: dict[str, str]) -> bytes:
        return "".join(
            f"WORLDSTREAM_ENOSPC_V1 {name}={value}\n" for name, value in values.items()
        ).encode("ascii")

    assert soak.parse_disk_full_container_output(marker_output(marker_values)) == (
        marker_values
    )
    malformed_outputs = (
        marker_output(
            {
                name: value
                for name, value in marker_values.items()
                if name != "filesystem_type"
            }
        ),
        marker_output(marker_values) + b"WORLDSTREAM_ENOSPC_V1 filesystem_type=ext4\n",
        marker_output(marker_values) + b"WORLDSTREAM_ENOSPC_V1 unexpected=value\n",
        marker_output(marker_values) + b"not-a-witness\n",
        marker_output(marker_values) + b"\xff",
    )
    for malformed in malformed_outputs:
        try:
            soak.parse_disk_full_container_output(malformed)
        except soak.SoakFailure:
            pass
        else:
            raise AssertionError("malformed disk-full container output was accepted")

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

#!/usr/bin/env python3
"""Executable tests for OCI filesystem policy and gate boundaries."""

from __future__ import annotations

import gzip
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
ENTRYPOINT = ROOT / "packaging/oci/entrypoint.sh"
DOCKERFILE = ROOT / "packaging/oci/Dockerfile"
METADATA = ROOT / "packaging/oci/oci-metadata.json"
RUNTIME_SMOKE = ROOT / "scripts/oci-runtime-smoke.sh"
VERIFY_LAYOUT = ROOT / "scripts/verify-oci-layout.py"
GATES = ROOT / "scripts/gates.py"


def load_layout_verifier():
    spec = importlib.util.spec_from_file_location(
        "worldstream_verify_oci_layout", VERIFY_LAYOUT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def oci_layout_fixture(
    tmp_path: Path,
    tamper: str | None = None,
    labels: dict[str, str] | None = None,
):
    media_manifest = "application/vnd.oci.image.manifest.v1+json"
    media_config = "application/vnd.oci.image.config.v1+json"
    media_layer = "application/vnd.oci.image.layer.v1.tar+gzip"

    def canonical(value: object) -> bytes:
        return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()

    def digest(value: bytes) -> str:
        return "sha256:" + hashlib.sha256(value).hexdigest()

    layer_unpacked = b"canonical synthetic OCI layer bytes\n"
    layer_blob = gzip.compress(layer_unpacked, mtime=0)
    layer_descriptor = {
        "mediaType": media_layer,
        "digest": digest(layer_blob),
        "size": len(layer_blob),
    }
    config = {
        "architecture": "amd64",
        "os": "linux",
        "config": {"Labels": labels or {}},
        "rootfs": {"type": "layers", "diff_ids": [digest(layer_unpacked)]},
    }
    if tamper == "wrong_diff_id":
        config["rootfs"]["diff_ids"] = ["sha256:" + "0" * 64]
    config_blob = canonical(config)
    config_descriptor = {
        "mediaType": media_config,
        "digest": digest(config_blob),
        "size": len(config_blob),
    }
    manifest = {
        "schemaVersion": 2,
        "config": config_descriptor,
        "layers": [layer_descriptor],
    }
    manifest_blob = canonical(manifest)
    manifest_descriptor = {
        "mediaType": media_manifest,
        "digest": digest(manifest_blob),
        "size": len(manifest_blob),
        "platform": {"architecture": "amd64", "os": "linux"},
    }
    index = {"schemaVersion": 2, "manifests": [manifest_descriptor]}
    if tamper == "extra_manifest":
        index["manifests"].append(dict(manifest_descriptor))
    if tamper == "descriptor_size":
        manifest["layers"][0]["size"] += 1
        manifest_blob = canonical(manifest)
        manifest_descriptor["digest"] = digest(manifest_blob)
        manifest_descriptor["size"] = len(manifest_blob)

    blobs = {
        config_descriptor["digest"].removeprefix("sha256:"): config_blob,
        manifest_descriptor["digest"].removeprefix("sha256:"): manifest_blob,
        layer_descriptor["digest"].removeprefix("sha256:"): layer_blob,
    }
    if tamper == "missing_layer":
        del blobs[layer_descriptor["digest"].removeprefix("sha256:")]
    elif tamper == "corrupt_layer":
        blobs[layer_descriptor["digest"].removeprefix("sha256:")] += b"tampered"
    elif tamper == "extra_blob":
        blobs["f" * 64] = b"unreferenced"

    files = {
        "oci-layout": canonical({"imageLayoutVersion": "1.0.0"}),
        "index.json": canonical(index),
        **{f"blobs/sha256/{name}": value for name, value in blobs.items()},
    }
    artifact = tmp_path / "image.oci.tar"
    with tarfile.open(artifact, "w:") as archive:
        for name, value in sorted(files.items()):
            member = tarfile.TarInfo(name)
            member.size = len(value)
            archive.addfile(member, __import__("io").BytesIO(value))
    return artifact, config_descriptor["digest"]


def test_oci_layout_verifier_binds_every_layer_and_closed_inventory(tmp_path):
    verifier = load_layout_verifier()
    artifact, tested_image_id = oci_layout_fixture(tmp_path)

    result = verifier.verify_artifact(artifact, tested_image_id)

    assert result["status"] == "pass"
    assert result["layer_count"] == 1
    assert result["layer_descriptors_bound"] is True
    assert result["rootfs_diff_ids_bound"] is True
    assert result["closed_blob_inventory"] is True


@pytest.mark.parametrize(
    "tamper",
    [
        "wrong_diff_id",
        "extra_manifest",
        "descriptor_size",
        "missing_layer",
        "corrupt_layer",
        "extra_blob",
    ],
)
def test_oci_layout_verifier_rejects_unbound_or_extra_content(tmp_path, tamper):
    verifier = load_layout_verifier()
    artifact, tested_image_id = oci_layout_fixture(tmp_path, tamper)

    with pytest.raises(verifier.VerificationError):
        verifier.verify_artifact(artifact, tested_image_id)


def run_policy(
    filesystem: str,
    data_dir: str = "/var/lib/worldstream",
    profile: str = "sqlite-bundled",
    mountinfo_failure: bool = False,
) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory(prefix="worldstream-oci-policy-") as temporary:
        root = Path(temporary)
        policy_dir = root / "worldstream-data"
        policy_dir.mkdir()
        fake_mountinfo = root / "mountinfo"
        if not mountinfo_failure:
            fake_mountinfo.write_text(
                f"32 24 0:30 / {policy_dir} rw,relatime - {filesystem} fixture rw\n",
                encoding="utf-8",
            )
        fake_daemon = root / "worldstreamd"
        fake_daemon.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        fake_daemon.chmod(0o755)
        script = ENTRYPOINT.read_text(encoding="utf-8")
        script = script.replace("/usr/local/bin/worldstreamd", str(fake_daemon))
        script = script.replace("/var/lib/worldstream", str(policy_dir))
        script = script.replace("/proc/self/mountinfo", str(fake_mountinfo))
        return subprocess.run(
            ["sh", "-eu", "-c", script],
            env={
                **os.environ,
                "PATH": f"{root}:{os.environ.get('PATH', '')}",
                "WORLDSTREAM__STORAGE__DATA_DIR": (
                    str(policy_dir) if data_dir == "/var/lib/worldstream" else data_dir
                ),
                "WORLDSTREAM__STORAGE__PROFILE": profile,
            },
            text=True,
            capture_output=True,
            check=False,
        )


def run_runtime_smoke_with_fake_docker(
    context: Path,
) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory(prefix="worldstream-oci-runtime-") as temporary:
        fake_bin = Path(temporary) / "bin"
        fake_bin.mkdir()
        fake_docker = fake_bin / "docker"
        fake_docker.write_text(
            "#!/bin/sh\n"
            'if [ "$1" = info ]; then exit 0; fi\n'
            'if [ "$1" = buildx ] && [ "$2" = inspect ]; then exit 0; fi\n'
            "exit 99\n",
            encoding="utf-8",
        )
        fake_docker.chmod(0o755)
        return subprocess.run(
            ["bash", str(RUNTIME_SMOKE), "--context", str(context)],
            cwd=ROOT,
            env={
                **os.environ,
                "PATH": f"{fake_bin}:/usr/bin:/bin",
            },
            text=True,
            capture_output=True,
            check=False,
        )


def run_real_docker_smoke() -> None:
    """Exercise both storage profiles in the exact packaged binaries."""

    docker = shutil.which("docker")
    if docker is None:
        print("OCI Docker integration: SKIP (docker unavailable)")
        return
    daemon = subprocess.run(
        [docker, "info"],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    buildx = subprocess.run(
        [docker, "buildx", "inspect"],
        check=False,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    if daemon.returncode != 0 or buildx.returncode != 0:
        print("OCI Docker integration: SKIP (Docker daemon/buildx unavailable)")
        return

    configured_binary_dir = os.environ.get("WORLDSTREAM_OCI_BINARY_DIR")
    binary_dir = (
        Path(configured_binary_dir)
        if configured_binary_dir
        else ROOT / "target/x86_64-unknown-linux-musl/release"
    )
    required_binaries = tuple(
        binary_dir / name for name in ("worldstreamd", "worldstreamctl")
    )
    if any(not path.is_file() for path in required_binaries):
        print(
            "OCI Docker integration: SKIP (exact linux/amd64 packaged binaries "
            f"are unavailable under {binary_dir})"
        )
        return

    base_image = (
        "alpine:3.22.1@sha256:"
        "4bcff63911fcb4448bd4fdacec207030997caf25e9bea4045fa6c8c44de311d1"
    )
    with tempfile.TemporaryDirectory(prefix="worldstream-oci-docker-") as temporary:
        temporary_root = Path(temporary)
        context = temporary_root / "context"
        context.mkdir()
        shutil.copy2(DOCKERFILE, context / "Dockerfile")
        shutil.copy2(ENTRYPOINT, context / "entrypoint.sh")
        (context / "oci-metadata.json").write_text(
            json.dumps(
                {
                    "artifact": "worldstream-oci/v1",
                    "base_image": base_image,
                    "version": "0.1.0",
                    "manifest_sha256": "0" * 64,
                    "profile": "oci-linux-amd64",
                    "target": "linux/amd64",
                    "runtime": {
                        "uid": 65532,
                        "gid": 65532,
                        "read_only_root": True,
                        "volume": "/var/lib/worldstream",
                        "healthcheck": (
                            "worldstreamctl --data-dir /var/lib/worldstream health"
                        ),
                        "sqlite_filesystems": ["ext4", "xfs"],
                        "reject_filesystems": [
                            "overlay",
                            "tmpfs",
                            "nfs",
                            "cifs",
                            "fuse",
                            "fuseblk",
                            "smb",
                        ],
                    },
                    "permissions": {"user": "65532:65532"},
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )
        for directory in (
            "bin",
            "ui",
            "sdk",
            "manifest",
            "examples/heist",
            "licenses",
        ):
            (context / directory).mkdir(parents=True, exist_ok=True)
        for source in required_binaries:
            destination = context / "bin" / source.name
            shutil.copy2(source, destination)
            destination.chmod(0o755)
        (context / "ui/index.html").write_text("<!doctype html>\n", encoding="utf-8")
        (context / "sdk/README.md").write_text("fixture\n", encoding="utf-8")
        (context / "manifest/compatibility.toml").write_text(
            "schema = 'fixture'\n", encoding="utf-8"
        )
        (context / "manifest/compatibility.json").write_text("{}\n", encoding="utf-8")
        (context / "examples/heist/client.py").write_text(
            "print('fixture')\n", encoding="utf-8"
        )
        (context / "licenses/LICENSE").write_text("fixture\n", encoding="utf-8")

        artifact = temporary_root / "worldstream-fixture-oci-linux-amd64.oci.tar"
        image_tag = f"worldstream-oci-artifact-test:{os.getpid()}"
        builder_name = temporary_root.name.replace("_", "-")
        created = subprocess.run(
            [
                docker,
                "buildx",
                "create",
                "--name",
                builder_name,
                "--driver",
                "docker-container",
            ],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
            timeout=60,
        )
        assert created.returncode == 0, created.stderr + created.stdout
        try:
            built = subprocess.run(
                [
                    docker,
                    "buildx",
                    "build",
                    "--builder",
                    builder_name,
                    "--platform",
                    "linux/amd64",
                    "--provenance=false",
                    "--output",
                    f"type=oci,dest={artifact},compression=gzip,force-compression=true",
                    "--load",
                    "--tag",
                    image_tag,
                    "--build-arg",
                    f"WORLDSTREAM_BASE_IMAGE={base_image}",
                    "--build-arg",
                    "VERSION=0.1.0",
                    "--build-arg",
                    f"MANIFEST_SHA256={'0' * 64}",
                    "--build-arg",
                    "SOURCE_DATE_EPOCH=0",
                    str(context),
                ],
                cwd=ROOT,
                text=True,
                capture_output=True,
                check=False,
                timeout=300,
            )
        finally:
            subprocess.run(
                [docker, "buildx", "rm", builder_name],
                cwd=ROOT,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
                timeout=60,
            )
        assert built.returncode == 0, built.stderr + built.stdout

        result = subprocess.run(
            [
                "bash",
                str(RUNTIME_SMOKE),
                "--context",
                str(context),
                "--artifact",
                str(artifact),
                "--image",
                image_tag,
                "--skip-build",
            ],
            cwd=ROOT,
            text=True,
            capture_output=True,
            check=False,
            timeout=300,
        )
        if result.returncode == 13:
            report = json.loads(result.stdout.strip().splitlines()[-1])
            assert report["status"] == "INCOMPLETE", report
            assert report["release_evidence"] is False, report
            assert report["negative_policy_checks"] == "PASS", report
            print(
                "OCI Docker integration: INCOMPLETE (real build and negative "
                "policy checks passed; local volume filesystem is outside ext4/xfs)"
            )
            return
        assert result.returncode == 0, result.stderr + result.stdout
        report = json.loads(result.stdout.strip().splitlines()[-1])
        assert report["status"] == "PASS", report
        assert report["release_evidence"] is False, report
        assert report["read_only_root"] is True, report
        assert report["non_root"] == "65532:65532", report
        assert report["authority_secret_source"] == (
            "owner-readable-read-only-volume-file"
        ), report
        assert report["standalone_config"] == "valid", report
        assert report["healthcheck_without_daemon"] == "rejected", report
        sqlite = report["profiles"]["sqlite-bundled"]
        assert sqlite["status"] == "pass", report
        assert sqlite["healthz"] == "pass", report
        assert sqlite["readyz"] == "pass", report
        assert sqlite["version"] == "pass", report
        assert sqlite["engine_identity"].startswith("sqlite/3.53.4;"), report
        postgres = report["profiles"]["postgres-primary"]
        assert postgres["status"] == "pass", report
        assert postgres["runtime_role_least_privilege"] is True, report
        assert postgres["network"] == "disabled-unix-socket", report
        assert report["secrets_emitted"] is False, report
        assert report["secret_scan"]["status"] == "pass", report
        assert report["secret_scan"]["secrets_emitted"] is False, report
        assert len(report["secret_scan"]["channels"]) == 11, report
        assert report["artifact_binding"]["status"] == "pass", report
        assert report["artifact_binding"]["artifact_sha256"].startswith("sha256:"), (
            report
        )
        assert (
            report["artifact_binding"]["artifact_image_config_digest"]
            == report["artifact_binding"]["tested_image_config_digest"]
        ), report
        assert report["artifact_binding"]["layer_count"] > 0, report
        assert report["artifact_binding"]["layer_descriptors_bound"] is True, report
        assert report["artifact_binding"]["rootfs_diff_ids_bound"] is True, report
        assert report["artifact_binding"]["closed_blob_inventory"] is True, report
        print(
            "OCI Docker integration: PASS (exact packaged linux/amd64 image; "
            "SQLite and PostgreSQL profiles)"
        )


def main() -> None:
    dockerfile = DOCKERFILE.read_text(encoding="utf-8")
    entrypoint = ENTRYPOINT.read_text(encoding="utf-8")
    metadata = json.loads(METADATA.read_text(encoding="utf-8"))
    assert "FROM ${WORLDSTREAM_BASE_IMAGE}" in dockerfile
    assert "USER 65532:65532" in dockerfile
    assert 'VOLUME ["/var/lib/worldstream"]' in dockerfile
    assert "HEALTHCHECK" in dockerfile and "worldstreamctl" in dockerfile
    assert 'io.worldstream.target="linux/amd64"' in dockerfile
    assert 'io.worldstream.read-only-root="true"' in dockerfile
    assert "ext4|xfs" in entrypoint
    assert "/proc/self/mountinfo" in entrypoint
    assert "unsupported storage profile" in entrypoint
    assert "mount identity could not be determined" in entrypoint
    runtime_smoke = RUNTIME_SMOKE.read_text(encoding="utf-8")
    layout_verifier = VERIFY_LAYOUT.read_text(encoding="utf-8")
    assert "ReadonlyRootfs" in runtime_smoke
    assert "read_only_root_write_succeeded" in runtime_smoke
    assert "healthcheck_succeeded_without_daemon" in runtime_smoke
    assert "WORLDSTREAM__AUTHORITY__BOOTSTRAP__SECRET_FILE" in runtime_smoke
    assert "config validate" in runtime_smoke
    assert "artifact_image_config_digest" in layout_verifier
    assert "tested_image_config_digest" in layout_verifier
    assert "layer_descriptors_bound" in layout_verifier
    assert "closed_blob_inventory" in layout_verifier
    assert "verify-oci-layout.py" in runtime_smoke
    assert "verify-secret-absence.py" in runtime_smoke
    assert "secrets.token_hex(16)" in runtime_smoke
    assert "validate_profile_probes" in runtime_smoke
    assert "skip_build_requires_artifact" in runtime_smoke
    assert "postgres:17.11-alpine@sha256:" in runtime_smoke
    assert '"postgres-primary"' in runtime_smoke
    assert "postgres_runtime_role_not_least_privileged" in runtime_smoke
    for runtime_role_witness in (
        "rolsuper",
        "rolcreaterole",
        "rolcreatedb",
        "rolreplication",
        "rolbypassrls",
        "has_database_privilege",
        "pg_auth_members",
        "has_schema_privilege",
        "pg_namespace",
        "pg_class",
        "pg_proc",
        "pg_type",
        "worldstream_schema_migrations",
    ):
        assert runtime_role_witness in runtime_smoke
    for filesystem in ("overlay", "tmpfs", "nfs", "cifs", "fuse", "fuseblk", "smb"):
        result = run_policy(filesystem)
        assert result.returncode == 78, (filesystem, result)
        assert "filesystem" in result.stderr
    wrong_path = run_policy("ext4", "/tmp")
    assert wrong_path.returncode == 78
    assert "data directory" in wrong_path.stderr
    unknown_profile = run_policy("ext4", profile="unknown-profile")
    assert unknown_profile.returncode == 78
    assert "unsupported storage profile" in unknown_profile.stderr
    postgres = run_policy("not-a-local-filesystem", profile="postgres-primary")
    assert postgres.returncode == 0, postgres
    mountinfo_failure = run_policy("ext4", mountinfo_failure=True)
    assert mountinfo_failure.returncode == 78
    assert "mount identity could not be determined" in mountinfo_failure.stderr
    ambiguous_ext_magic = run_policy("ext2/ext3")
    assert ambiguous_ext_magic.returncode == 78
    assert "outside the ext4/xfs allow-list" in ambiguous_ext_magic.stderr
    allowed = run_policy("ext4")
    assert allowed.returncode == 0, allowed
    unresolved_context = run_runtime_smoke_with_fake_docker(ROOT / "packaging/oci")
    assert unresolved_context.returncode == 13, unresolved_context
    unresolved_report = json.loads(unresolved_context.stdout)
    assert unresolved_report == {
        "schema": "worldstream/oci-runtime-smoke/v1",
        "status": "INCOMPLETE",
        "reason": "pinned_base_image_required",
        "release_evidence": False,
    }
    assert metadata["base_image"] == {
        "digest": "",
        "status": "must_be_pinned_by_release_evidence",
    }
    assert metadata["image"] == {
        "architecture": "amd64",
        "os": "linux",
        "read_only_root": True,
        "uid": 65532,
        "gid": 65532,
        "healthcheck": "worldstreamctl --data-dir /var/lib/worldstream health",
    }
    gates = GATES.read_text(encoding="utf-8")
    assert "oci-context-dry-run" in gates
    assert "oci-template-syntax" in gates
    assert "--check" in gates
    assert "image digest and runtime evidence remain external" in gates
    print("OCI policy and manifest-gate boundary: PASS")
    if os.environ.get("WORLDSTREAM_RUN_OCI_DOCKER") == "1":
        run_real_docker_smoke()


if __name__ == "__main__":
    main()

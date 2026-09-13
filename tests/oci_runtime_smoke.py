#!/usr/bin/env python3
"""Executable tests for OCI filesystem policy and gate boundaries."""

from __future__ import annotations

import base64
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
VERIFY_LOCAL_VOLUME = ROOT / "scripts/verify-local-docker-volume.py"
GATES = ROOT / "scripts/gates.py"
FIXTURE_BASE_IMAGE = (
    (ROOT / "packaging/oci/base-image.txt").read_text(encoding="utf-8").strip()
)
BASH = shutil.which("bash") or "bash"
RUNTIME_ROLE_SQL_FRAGMENTS = (
    "role.rolsuper::text",
    "role.rolcreaterole::text",
    "role.rolcreatedb::text",
    "role.rolreplication::text",
    "role.rolbypassrls::text",
    "has_database_privilege(current_user, current_database(), 'CREATE')",
    "pg_catalog.pg_auth_members",
    "has_schema_privilege(current_user, 'public', 'CREATE')",
    "pg_catalog.pg_namespace",
    "pg_catalog.pg_class",
    "pg_catalog.pg_proc",
    "pg_catalog.pg_type",
    "'public.worldstream_schema_migrations', 'INSERT'",
    "'public.worldstream_schema_migrations', 'UPDATE'",
    "'public.worldstream_schema_migrations', 'DELETE'",
    "'public.worldstream_schema_migrations', 'TRUNCATE'",
    "protected_table.table_name, 'INSERT'",
    "protected_table.table_name, 'UPDATE'",
    "protected_table.table_name, 'DELETE'",
    "protected_table.table_name, 'TRUNCATE'",
    "'public.worldstream_frames', 'DELETE'",
)


def assert_runtime_role_contract(source: str) -> None:
    start = source.index("runtime_role_admission_sql() {")
    end = source.index("\n}\n", start)
    query = source[start:end]
    expected = "false|" * 17 + "false"
    assert f'RUNTIME_ROLE_ADMISSION_EXPECTED="{expected}"' in source
    assert query.count("|| '|' ||") == 17
    for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
        assert fragment in query
    assert "unnest(ARRAY['INSERT'" not in query


def embedded_python(source: str, marker: str) -> str:
    start = source.index(marker) + len(marker)
    end = source.index("\nPY", start)
    return source[start:end] + "\n"


def load_layout_verifier():
    spec = importlib.util.spec_from_file_location(
        "worldstream_verify_oci_layout", VERIFY_LAYOUT
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def load_local_volume_verifier():
    spec = importlib.util.spec_from_file_location(
        "worldstream_verify_local_docker_volume", VERIFY_LOCAL_VOLUME
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def docker_fixture_build_identity() -> dict[str, object]:
    source_revision = subprocess.check_output(
        ["git", "rev-parse", "--verify", "HEAD^{commit}"],
        cwd=ROOT,
        text=True,
    ).strip()
    manifest_json = (ROOT / "compatibility.json").read_bytes()
    manifest = json.loads(manifest_json)
    observed_build_environment = {
        "runner": {
            "provider": "local-test",
            "os": "local",
            "architecture": "local",
            "image": "local",
            "image_version": "local",
        },
        "rustc": None,
        "bundled_sqlite": None,
        "final_linker": None,
    }
    encoded_environment = base64.b64encode(
        (
            json.dumps(observed_build_environment, indent=2, sort_keys=True) + "\n"
        ).encode()
    ).decode("ascii")
    return {
        "version": manifest["contracts"]["product"],
        "manifest_sha256": hashlib.sha256(manifest_json).hexdigest(),
        "source_revision": source_revision,
        "build_identity_sha256": "sha256:" + "0" * 64,
        "observed_build_environment": observed_build_environment,
        "build_environment_base64": encoded_environment,
    }


def docker_fixture_build_args(identity: dict[str, object]) -> list[str]:
    return [
        "--build-arg",
        f"VERSION={identity['version']}",
        "--build-arg",
        f"MANIFEST_SHA256={identity['manifest_sha256']}",
        "--build-arg",
        f"SOURCE_REVISION={identity['source_revision']}",
        "--build-arg",
        f"BUILD_IDENTITY_SHA256={identity['build_identity_sha256']}",
        "--build-arg",
        f"BUILD_ENVIRONMENT_BASE64={identity['build_environment_base64']}",
        "--build-arg",
        "SOURCE_DATE_EPOCH=0",
    ]


def docker_fixture_metadata(identity: dict[str, object]) -> dict[str, object]:
    return {
        "artifact": "worldstream-oci/v1",
        "base_image": FIXTURE_BASE_IMAGE,
        **identity,
        "profile": "oci-linux-amd64",
        "target": "linux/amd64",
        "runtime": {
            "uid": 65532,
            "gid": 65532,
            "read_only_root": True,
            "volume": "/var/lib/worldstream",
            "healthcheck": "worldstreamctl --data-dir /var/lib/worldstream health",
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
    }


def test_real_docker_fixture_carries_complete_build_identity():
    identity = docker_fixture_build_identity()

    assert len(identity["source_revision"]) == 40
    assert all(
        character in "0123456789abcdef" for character in identity["source_revision"]
    )
    assert len(identity["manifest_sha256"]) == 64
    assert identity["build_identity_sha256"].startswith("sha256:")
    arguments = docker_fixture_build_args(identity)
    for name in (
        "VERSION",
        "MANIFEST_SHA256",
        "SOURCE_REVISION",
        "BUILD_IDENTITY_SHA256",
        "BUILD_ENVIRONMENT_BASE64",
        "SOURCE_DATE_EPOCH",
    ):
        assert any(argument.startswith(f"{name}=") for argument in arguments)
    assert (
        docker_fixture_metadata(identity)["source_revision"]
        == identity["source_revision"]
    )


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


def local_volume_record(**overrides):
    record = {
        "Name": "worldstream",
        "Driver": "local",
        "Scope": "local",
        "Options": None,
        "Mountpoint": "/var/lib/docker/volumes/worldstream/_data",
    }
    record.update(overrides)
    return [record]


def test_local_volume_verifier_accepts_only_unconfigured_local_driver():
    verifier = load_local_volume_verifier()

    assert verifier.validate_volume_records(local_volume_record(), "worldstream") == {
        "driver": "local",
        "driver_options": {},
        "scope": "local",
    }


@pytest.mark.parametrize(
    ("overrides", "message"),
    [
        ({"Driver": "nfs-plugin"}, "driver is not local"),
        ({"Scope": "global"}, "scope is not local"),
        (
            {
                "Options": {
                    "type": "nfs",
                    "o": "addr=192.0.2.1",
                    "device": ":/worldstream",
                }
            },
            "driver options",
        ),
        ({"Mountpoint": "relative/path"}, "mountpoint is invalid"),
        ({"Name": "other"}, "wrong volume"),
    ],
)
def test_local_volume_verifier_rejects_unproven_locality(overrides, message):
    verifier = load_local_volume_verifier()

    with pytest.raises(verifier.VolumeVerificationError, match=message):
        verifier.validate_volume_records(
            local_volume_record(**overrides), "worldstream"
        )


def run_policy(
    filesystem: str,
    data_dir: str = "/var/lib/worldstream",
    profile: str = "sqlite-bundled",
    mountinfo_failure: bool = False,
    mount_device: str = "254:1",
    mount_source: str = "/dev/vda1",
) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory(prefix="worldstream-oci-policy-") as temporary:
        root = Path(temporary)
        policy_dir = root / "worldstream-data"
        policy_dir.mkdir()
        fake_mountinfo = root / "mountinfo"
        if not mountinfo_failure:
            fake_mountinfo.write_text(
                f"32 24 {mount_device} / {policy_dir} rw,relatime "
                f"- {filesystem} {mount_source} rw\n",
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
            [BASH, str(RUNTIME_SMOKE), "--context", str(context)],
            cwd=ROOT,
            env={
                **os.environ,
                "PATH": f"{fake_bin}:/usr/bin:/bin",
            },
            text=True,
            capture_output=True,
            check=False,
        )


def test_real_docker_fixture_metadata_passes_runtime_preflight(tmp_path):
    context = tmp_path / "context"
    context.mkdir()
    for required in ("Dockerfile", "entrypoint.sh"):
        (context / required).write_text("fixture\n", encoding="utf-8")
    (context / "oci-metadata.json").write_text(
        json.dumps(docker_fixture_metadata(docker_fixture_build_identity())),
        encoding="utf-8",
    )

    result = run_runtime_smoke_with_fake_docker(context)

    assert result.returncode == 14, result
    report = json.loads(result.stdout.strip().splitlines()[-1])
    assert report["status"] == "FAIL", report
    assert report["reason"] == "image_build", report


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

    base_image = FIXTURE_BASE_IMAGE
    identity = docker_fixture_build_identity()
    with tempfile.TemporaryDirectory(prefix="worldstream-oci-docker-") as temporary:
        temporary_root = Path(temporary)
        context = temporary_root / "context"
        context.mkdir()
        shutil.copy2(DOCKERFILE, context / "Dockerfile")
        shutil.copy2(ENTRYPOINT, context / "entrypoint.sh")
        (context / "oci-metadata.json").write_text(
            json.dumps(
                docker_fixture_metadata(identity),
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
            "metadata",
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
        shutil.copy2(ROOT / "compatibility.toml", context / "manifest")
        shutil.copy2(ROOT / "compatibility.json", context / "manifest")
        (context / "examples/heist/client.py").write_text(
            "print('fixture')\n", encoding="utf-8"
        )
        for notice in (ROOT / "licenses").iterdir():
            if notice.is_file():
                shutil.copy2(notice, context / "licenses" / notice.name)

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
                    *docker_fixture_build_args(identity),
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
                BASH,
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
                "policy checks passed; local ext4/xfs block identity is unavailable)"
            )
            return
        assert result.returncode == 0, result.stderr + result.stdout
        report = json.loads(result.stdout.strip().splitlines()[-1])
        assert report["status"] == "PASS", report
        assert report["release_evidence"] is False, report
        assert report["read_only_root"] is True, report
        assert report["non_root"] == "65532:65532", report
        volume = report["sqlite_volume"]
        assert volume["path"] == "/var/lib/worldstream", report
        assert volume["type"] == "docker-volume", report
        assert volume["driver"] == "local", report
        assert volume["scope"] == "local", report
        assert volume["driver_options"] == {}, report
        assert int(volume["mount_device"].split(":", maxsplit=1)[0]) > 0, report
        assert volume["filesystem"] in {"ext4", "xfs"}, report
        assert volume["mount_source"].startswith("/dev/"), report
        assert volume["locality"] == "local-block-device", report
        assert report["rejected_layouts"] == [
            "tmpfs",
            "wrong-data-directory",
            "network-configured-volume",
        ], report
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
        assert len(report["secret_scan"]["channels"]) == 28, report
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


@pytest.mark.parametrize(
    ("field", "raw", "reason"),
    [
        ("health", '{"status":"bad","status":"ok"}', "runtime_health"),
        ("ready", '{"status":"bad","status":"ready"}', "runtime_ready"),
        ("version", '{"value":0,"value":1}', "runtime_version"),
        (
            "control_version",
            '{"value":0,"value":1}',
            "runtime_control_version",
        ),
        ("health", '{"status":NaN}', "runtime_health"),
        ("health", '{"status":Infinity}', "runtime_health"),
        ("health", '{"status":-Infinity}', "runtime_health"),
    ],
)
def test_runtime_profile_json_is_strict_before_it_can_cause_pass(
    tmp_path: Path, field: str, raw: str, reason: str
):
    source = RUNTIME_SMOKE.read_text(encoding="utf-8")
    script = embedded_python(source, "    \"$source_revision\" <<'PY'\n")
    paths = {}
    for name in ("health", "ready", "version", "control_version"):
        path = tmp_path / f"{name}.json"
        path.write_text(raw if name == field else "{}", encoding="utf-8")
        paths[name] = path
    result = subprocess.run(
        [
            sys.executable,
            "-",
            str(ROOT / "scripts/package.py"),
            str(ROOT / "compatibility.json"),
            str(paths["health"]),
            str(paths["ready"]),
            str(paths["version"]),
            str(paths["control_version"]),
            "sqlite-bundled",
            "",
            "0" * 40,
        ],
        input=script,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0
    assert f"{reason}_not_strict_json" in result.stderr


def test_oci_runtime_capture_kills_oversized_output_before_allocation(tmp_path: Path):
    source = RUNTIME_SMOKE.read_text(encoding="utf-8")
    script = embedded_python(
        source,
        '  "$python_bin" - "$output_path" 16777216 "$timeout_seconds" "$@" <<\'PY\'\n',
    )
    output = tmp_path / "oversized.json"
    result = subprocess.run(
        [
            sys.executable,
            "-",
            str(output),
            "1024",
            "10",
            sys.executable,
            "-c",
            "import os; os.write(1, b'x' * (1024 * 1024))",
        ],
        input=script,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0
    assert not output.exists()


def valid_image_inspection() -> dict[str, object]:
    return {
        "Architecture": "amd64",
        "Config": {
            "Healthcheck": {"Test": ["CMD", "worldstreamctl", "health"]},
            "Labels": {
                "io.worldstream.read-only-root": "true",
                "io.worldstream.sqlite-filesystems": "ext4,xfs",
                "io.worldstream.target": "linux/amd64",
            },
            "User": "65532:65532",
            "Volumes": {"/var/lib/worldstream": {}},
        },
        "Os": "linux",
    }


def image_configuration_inputs(
    tmp_path: Path, inspection_raw: str, *, database_digest: str | None = None
) -> list[str]:
    inspection = tmp_path / "image-inspection.json"
    inspection.write_text(inspection_raw, encoding="utf-8")
    notice_manifest = ROOT / "licenses/THIRD-PARTY-NOTICES.json"
    expected = json.loads(notice_manifest.read_text(encoding="utf-8"))["components"][
        "oci_base"
    ]["installed_database_sha256"].removeprefix("sha256:")
    database = tmp_path / "base-package-database.sha256"
    database.write_text(
        f"{database_digest or expected}  /lib/apk/db/installed\n", encoding="ascii"
    )
    return [
        sys.executable,
        "-",
        str(ROOT / "scripts/package.py"),
        str(inspection),
        str(database),
        str(notice_manifest),
    ]


@pytest.mark.parametrize(
    "raw",
    [
        ('{"Architecture":"amd64","Config":{"User":"0"},"Config":{},"Os":"linux"}'),
        '{"Architecture":"amd64","Config":{},"Os":"linux","size":NaN}',
        '{"Architecture":"amd64","Config":{},"Os":"linux","size":Infinity}',
        '{"Architecture":"amd64","Config":{},"Os":"linux","size":-Infinity}',
    ],
)
def test_image_inspection_is_strict_before_it_can_cause_pass(tmp_path: Path, raw: str):
    source = RUNTIME_SMOKE.read_text(encoding="utf-8")
    script = embedded_python(
        source,
        "  \"$context_dir/licenses/THIRD-PARTY-NOTICES.json\" 2>/dev/null <<'PY'\n",
    )
    result = subprocess.run(
        image_configuration_inputs(tmp_path, raw),
        input=script,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0


def test_valid_image_inspection_passes_the_strict_boundary(tmp_path: Path):
    source = RUNTIME_SMOKE.read_text(encoding="utf-8")
    script = embedded_python(
        source,
        "  \"$context_dir/licenses/THIRD-PARTY-NOTICES.json\" 2>/dev/null <<'PY'\n",
    )
    result = subprocess.run(
        image_configuration_inputs(tmp_path, json.dumps(valid_image_inspection())),
        input=script,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_image_configuration_rejects_base_package_database_drift(tmp_path: Path):
    source = RUNTIME_SMOKE.read_text(encoding="utf-8")
    script = embedded_python(
        source,
        "  \"$context_dir/licenses/THIRD-PARTY-NOTICES.json\" 2>/dev/null <<'PY'\n",
    )
    result = subprocess.run(
        image_configuration_inputs(
            tmp_path,
            json.dumps(valid_image_inspection()),
            database_digest="0" * 64,
        ),
        input=script,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0


@pytest.mark.parametrize(
    "raw",
    [
        '{"status":"bad","status":"ok"}',
        '{"status":"ok","unused":NaN}',
        '{"status":"ok","unused":Infinity}',
        '{"status":"ok","unused":-Infinity}',
    ],
)
def test_packaged_admin_output_is_strict_before_it_can_cause_pass(
    tmp_path: Path, raw: str
):
    source = RUNTIME_SMOKE.read_text(encoding="utf-8")
    script = embedded_python(
        source,
        '"$python_bin" - "$SCRIPT_DIR/package.py" "$migrate_output" "$verify_output" <<\'PY\'\n',
    )
    migrate = tmp_path / "migrate.json"
    migrate.write_text(raw, encoding="utf-8")
    verify = tmp_path / "verify.json"
    verify.write_text(
        json.dumps(
            {
                "connection": "direct-admin",
                "operation": "verify",
                "release_evidence": False,
                "status": "ok",
            }
        ),
        encoding="utf-8",
    )
    result = subprocess.run(
        [
            sys.executable,
            "-",
            str(ROOT / "scripts/package.py"),
            str(migrate),
            str(verify),
        ],
        input=script,
        text=True,
        capture_output=True,
        check=False,
    )
    assert result.returncode != 0


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
    assert "mount source is not a local block device" in entrypoint
    assert "mount device is not a local block device" in entrypoint
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
    assert "verify-local-docker-volume.py" in runtime_smoke
    assert "verify-secret-absence.py" in runtime_smoke
    assert "secrets.token_hex(16)" in runtime_smoke
    assert "validate_profile_probes" in runtime_smoke
    assert "expected_control_version" in runtime_smoke
    assert '"binary": "worldstreamctl"' in runtime_smoke
    assert '"source_revision": expected_source_revision' in runtime_smoke
    assert "sqlite_control_version_json" in runtime_smoke
    assert "postgres_control_version_json" in runtime_smoke
    assert "image_inspection" in runtime_smoke
    assert "subprocess.check_output" not in runtime_smoke
    assert "skip_build_requires_artifact" in runtime_smoke
    assert "postgres:17.11-alpine@sha256:" in runtime_smoke
    assert '"postgres-primary"' in runtime_smoke
    assert "postgres_runtime_role_not_least_privileged" in runtime_smoke
    assert "NOREPLICATION NOBYPASSRLS" in runtime_smoke
    assert_runtime_role_contract(runtime_smoke)
    for fragment in RUNTIME_ROLE_SQL_FRAGMENTS:
        try:
            assert_runtime_role_contract(runtime_smoke.replace(fragment, "mutated"))
        except AssertionError:
            pass
        else:
            raise AssertionError(f"runtime role witness mutation survived: {fragment}")
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
    network_source = run_policy("ext4", mount_source="server:/worldstream")
    assert network_source.returncode == 78
    assert "mount source is not a local block device" in network_source.stderr
    virtual_device = run_policy("ext4", mount_device="0:30")
    assert virtual_device.returncode == 78
    assert "mount device is not a local block device" in virtual_device.stderr
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

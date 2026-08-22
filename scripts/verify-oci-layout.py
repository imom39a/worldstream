#!/usr/bin/env python3
"""Verify one closed linux/amd64 OCI image archive against a tested image ID."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import stat
import sys
import tarfile
import zlib
from pathlib import Path
from typing import Any, BinaryIO

MAX_ARTIFACT_BYTES = 8 * 1024 * 1024 * 1024
MAX_BLOB_BYTES = MAX_ARTIFACT_BYTES
MAX_JSON_BYTES = 16 * 1024 * 1024
MAX_LAYER_COUNT = 256
MAX_MEMBERS = 20_000
MAX_NAME_BYTES = 4_096
MAX_UNPACKED_LAYER_BYTES = 32 * 1024 * 1024 * 1024
DIGEST = re.compile(r"sha256:([0-9a-f]{64})")
OCI_MANIFEST = "application/vnd.oci.image.manifest.v1+json"
OCI_CONFIG = "application/vnd.oci.image.config.v1+json"
OCI_LAYER = "application/vnd.oci.image.layer.v1.tar"
OCI_LAYER_GZIP = "application/vnd.oci.image.layer.v1.tar+gzip"


class VerificationError(RuntimeError):
    """The archive is not the exact closed OCI image claimed by the caller."""


def reject(reason: str) -> None:
    raise VerificationError(reason)


def sha256_bytes(value: bytes) -> str:
    return "sha256:" + hashlib.sha256(value).hexdigest()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def safe_member(member: tarfile.TarInfo) -> bool:
    name = member.name
    parts = name.split("/")
    try:
        name_bytes = name.encode("utf-8")
    except UnicodeEncodeError:
        return False
    return (
        bool(name)
        and len(name_bytes) <= MAX_NAME_BYTES
        and not name.startswith("/")
        and not name.endswith("/")
        and "\\" not in name
        and all(part not in {"", ".", ".."} for part in parts)
        and all(ord(character) >= 32 and ord(character) != 127 for character in name)
        and (member.isfile() or member.isdir())
    )


def strict_json(value: bytes, reason: str) -> dict[str, Any]:
    def object_pairs(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for key, item in pairs:
            if key in result:
                reject(reason)
            result[key] = item
        return result

    try:
        decoded = json.loads(value, object_pairs_hook=object_pairs)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise VerificationError(reason) from error
    if not isinstance(decoded, dict):
        reject(reason)
    return decoded


def regular_members(archive: tarfile.TarFile) -> dict[str, tarfile.TarInfo]:
    members: dict[str, tarfile.TarInfo] = {}
    for member in archive:
        if len(members) >= MAX_MEMBERS:
            reject("artifact_member_limit_exceeded")
        if not safe_member(member) or member.name in members:
            reject("artifact_member_inventory_invalid")
        members[member.name] = member
    if not members:
        reject("artifact_is_empty")
    return members


def member_stream(
    archive: tarfile.TarFile,
    members: dict[str, tarfile.TarInfo],
    name: str,
    maximum: int,
) -> tuple[tarfile.TarInfo, BinaryIO]:
    member = members.get(name)
    if (
        member is None
        or not member.isfile()
        or isinstance(member.size, bool)
        or not (0 < member.size <= maximum)
    ):
        reject("referenced_blob_missing_or_invalid")
    source = archive.extractfile(member)
    if source is None:
        reject("referenced_blob_unreadable")
    return member, source


def member_bytes(
    archive: tarfile.TarFile,
    members: dict[str, tarfile.TarInfo],
    name: str,
) -> bytes:
    member, source = member_stream(archive, members, name, MAX_JSON_BYTES)
    try:
        value = source.read(MAX_JSON_BYTES + 1)
    finally:
        source.close()
    if len(value) != member.size or len(value) > MAX_JSON_BYTES:
        reject("json_blob_size_mismatch")
    return value


def descriptor_member(
    archive: tarfile.TarFile,
    members: dict[str, tarfile.TarInfo],
    descriptor: object,
    expected_media_type: str,
    maximum: int,
) -> tuple[str, tarfile.TarInfo]:
    if not isinstance(descriptor, dict):
        reject("descriptor_not_an_object")
    match = DIGEST.fullmatch(str(descriptor.get("digest", "")))
    size = descriptor.get("size")
    if (
        descriptor.get("mediaType") != expected_media_type
        or match is None
        or isinstance(size, bool)
        or not isinstance(size, int)
        or not (0 < size <= maximum)
    ):
        reject("descriptor_identity_invalid")
    name = f"blobs/sha256/{match.group(1)}"
    member, source = member_stream(archive, members, name, maximum)
    source.close()
    if member.size != size:
        reject("descriptor_size_mismatch")
    return name, member


def descriptor_json(
    archive: tarfile.TarFile,
    members: dict[str, tarfile.TarInfo],
    descriptor: object,
    expected_media_type: str,
    reason: str,
) -> tuple[str, dict[str, Any]]:
    name, _member = descriptor_member(
        archive, members, descriptor, expected_media_type, MAX_JSON_BYTES
    )
    value = member_bytes(archive, members, name)
    if sha256_bytes(value) != descriptor["digest"]:  # type: ignore[index]
        reject("descriptor_digest_mismatch")
    return name, strict_json(value, reason)


def hash_stream(source: BinaryIO, maximum: int) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    while chunk := source.read(1024 * 1024):
        size += len(chunk)
        if size > maximum:
            reject("layer_unpacked_size_limit_exceeded")
        digest.update(chunk)
    return "sha256:" + digest.hexdigest(), size


def verify_layer(
    archive: tarfile.TarFile,
    members: dict[str, tarfile.TarInfo],
    descriptor: object,
    expected_diff_id: str,
) -> str:
    if not isinstance(descriptor, dict):
        reject("layer_descriptor_not_an_object")
    media_type = descriptor.get("mediaType")
    if media_type not in {OCI_LAYER, OCI_LAYER_GZIP}:
        reject("layer_media_type_not_supported")
    name, member = descriptor_member(
        archive, members, descriptor, media_type, MAX_BLOB_BYTES
    )
    _member, source = member_stream(archive, members, name, MAX_BLOB_BYTES)
    try:
        blob_digest, blob_size = hash_stream(source, MAX_BLOB_BYTES)
    finally:
        source.close()
    if blob_size != member.size or blob_digest != descriptor["digest"]:
        reject("layer_blob_digest_mismatch")

    _member, source = member_stream(archive, members, name, MAX_BLOB_BYTES)
    try:
        if media_type == OCI_LAYER_GZIP:
            unpacked: BinaryIO = gzip.GzipFile(fileobj=source, mode="rb")
            try:
                diff_id, _unpacked_size = hash_stream(
                    unpacked, MAX_UNPACKED_LAYER_BYTES
                )
            finally:
                unpacked.close()
        else:
            diff_id, _unpacked_size = hash_stream(source, MAX_UNPACKED_LAYER_BYTES)
    except (EOFError, gzip.BadGzipFile, zlib.error) as error:
        raise VerificationError("layer_compression_invalid") from error
    finally:
        source.close()
    if DIGEST.fullmatch(expected_diff_id) is None or diff_id != expected_diff_id:
        reject("layer_diff_id_mismatch")
    return name


def verify_artifact(path: Path, tested_image_id: str | None = None) -> dict[str, Any]:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise VerificationError("artifact_unreadable") from error
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or not (0 < metadata.st_size <= MAX_ARTIFACT_BYTES)
        or (tested_image_id is not None and DIGEST.fullmatch(tested_image_id) is None)
    ):
        reject("artifact_or_tested_image_identity_invalid")

    try:
        with tarfile.open(path, "r:") as archive:
            members = regular_members(archive)
            layout = strict_json(
                member_bytes(archive, members, "oci-layout"), "oci_layout_invalid"
            )
            index = strict_json(
                member_bytes(archive, members, "index.json"), "oci_index_invalid"
            )
            if layout != {"imageLayoutVersion": "1.0.0"}:
                reject("oci_layout_version_invalid")
            manifests = index.get("manifests")
            if index.get("schemaVersion") != 2 or not isinstance(manifests, list):
                reject("oci_index_shape_invalid")
            if len(manifests) != 1:
                reject("oci_index_must_contain_one_image")
            manifest_descriptor = manifests[0]
            if not isinstance(manifest_descriptor, dict) or manifest_descriptor.get(
                "platform"
            ) != {"architecture": "amd64", "os": "linux"}:
                reject("oci_image_platform_invalid")
            manifest_name, manifest = descriptor_json(
                archive,
                members,
                manifest_descriptor,
                OCI_MANIFEST,
                "oci_manifest_invalid",
            )
            if manifest.get("schemaVersion") != 2:
                reject("oci_manifest_shape_invalid")
            config_descriptor = manifest.get("config")
            config_name, config = descriptor_json(
                archive,
                members,
                config_descriptor,
                OCI_CONFIG,
                "oci_config_invalid",
            )
            config_digest = config_descriptor.get("digest")  # type: ignore[union-attr]
            rootfs = config.get("rootfs")
            layers = manifest.get("layers")
            diff_ids = rootfs.get("diff_ids") if isinstance(rootfs, dict) else None
            if tested_image_id is None:
                tested_image_id = config_digest
            if (
                config.get("architecture") != "amd64"
                or config.get("os") != "linux"
                or not isinstance(rootfs, dict)
                or rootfs.get("type") != "layers"
                or not isinstance(layers, list)
                or not isinstance(diff_ids, list)
                or not (0 < len(layers) <= MAX_LAYER_COUNT)
                or len(layers) != len(diff_ids)
                or config_digest != tested_image_id
            ):
                reject("oci_config_rootfs_identity_invalid")

            referenced = {"oci-layout", "index.json", manifest_name, config_name}
            for descriptor, expected_diff_id in zip(layers, diff_ids, strict=True):
                if not isinstance(expected_diff_id, str):
                    reject("layer_diff_id_invalid")
                referenced.add(
                    verify_layer(archive, members, descriptor, expected_diff_id)
                )
            regular_files = {
                name for name, member in members.items() if member.isfile()
            }
            if regular_files != referenced:
                reject("oci_archive_contains_unreferenced_files")
    except (OSError, tarfile.TarError) as error:
        raise VerificationError("artifact_tar_invalid") from error

    return {
        "status": "pass",
        "artifact": path.name,
        "artifact_sha256": sha256_file(path),
        "artifact_size_bytes": metadata.st_size,
        "artifact_image_manifest_digest": manifest_descriptor["digest"],
        "artifact_image_config_digest": config_digest,
        "tested_image_config_digest": tested_image_id,
        "layer_count": len(layers),
        "layer_descriptors_bound": True,
        "rootfs_diff_ids_bound": True,
        "closed_blob_inventory": True,
    }


def verify_artifact_structure(path: Path) -> dict[str, Any]:
    """Verify a closed OCI archive when no separate runtime image ID is available.

    Release assembly uses this structural path in addition to the signed OCI
    platform report's exact archive digest-and-size binding. Runtime diagnostics
    continue to call :func:`verify_artifact` with the independently observed
    image config digest.
    """

    return verify_artifact(path)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    command.add_argument("--artifact", type=Path, required=True)
    command.add_argument("--tested-image-id", required=True)
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        result = verify_artifact(args.artifact, args.tested_image_id)
    except VerificationError as error:
        print(f"OCI artifact verification failed: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

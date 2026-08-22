#!/usr/bin/env python3
"""Generate the deterministic third-party notice bundle from pinned inputs."""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.util
import io
import json
import os
import re
import subprocess
import sys
import tarfile
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[1]
IDENTITY_PATH = ROOT / "scripts/release_build_identity.py"
MANIFEST_PATH = ROOT / "licenses/THIRD-PARTY-NOTICES.json"
NOTICES_PATH = ROOT / "licenses/THIRD-PARTY-NOTICES.txt"
SPDX_LICENSE_LIST_REVISION = "c4a7237ec8f4654e867546f9f409749300f1bf4c"
SPDX_LICENSE_URL = (
    "https://raw.githubusercontent.com/spdx/license-list-data/"
    f"{SPDX_LICENSE_LIST_REVISION}/text/{{license_id}}.txt"
)
NOTICE_SCHEMA = "worldstream/third-party-notices/v1"
SHA256_REF = re.compile(r"sha256:[0-9a-f]{64}\Z")
INTEGRITY = re.compile(r"sha512-[A-Za-z0-9+/]+={0,2}\Z")
LICENSE_TOKEN = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+-]*")


class NoticeError(RuntimeError):
    """Raised when an upstream notice fact cannot be bound exactly."""


def fail(message: str) -> None:
    raise NoticeError(message)


def identity_module():
    spec = importlib.util.spec_from_file_location(
        "worldstream_notice_identity", IDENTITY_PATH
    )
    if spec is None or spec.loader is None:
        fail("cannot load release build identity")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def sha256(content: bytes) -> str:
    return "sha256:" + hashlib.sha256(content).hexdigest()


def canonical_json(value: object) -> bytes:
    return (
        json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"
    ).encode()


def exact_url(url: str, *, expected_sha256: str | None = None) -> bytes:
    try:
        with urllib.request.urlopen(url, timeout=60) as response:
            content = response.read()
    except OSError as error:
        raise NoticeError(f"cannot download pinned notice input {url}") from error
    if not content:
        fail(f"pinned notice input is empty: {url}")
    if (
        expected_sha256 is not None
        and hashlib.sha256(content).hexdigest() != expected_sha256
    ):
        fail(f"pinned notice input digest differs: {url}")
    return content


def normalized_notice(content: bytes, label: str) -> bytes:
    try:
        text = content.decode("utf-8-sig")
    except UnicodeError as error:
        raise NoticeError(f"notice is not UTF-8: {label}") from error
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    text = "\n".join(line.rstrip(" \t") for line in text.split("\n")).rstrip() + "\n"
    if "===== BEGIN NOTICE " in text or "===== END NOTICE " in text:
        fail(f"notice contains a reserved delimiter: {label}")
    return text.encode()


def add_notice(notices: dict[str, bytes], content: bytes, label: str) -> str:
    normalized = normalized_notice(content, label)
    identifier = sha256(normalized)
    previous = notices.setdefault(identifier, normalized)
    if previous != normalized:
        fail(f"notice digest collision: {label}")
    return identifier


def license_ids(expression: str) -> list[str]:
    identifiers = [
        token
        for token in LICENSE_TOKEN.findall(expression)
        if token not in {"AND", "OR", "WITH"}
    ]
    if not identifiers:
        fail(f"declared license has no identifiers: {expression!r}")
    return sorted(set(identifiers))


def standard_license_notice(
    notices: dict[str, bytes], cache: dict[str, str], license_id: str
) -> str:
    existing = cache.get(license_id)
    if existing is not None:
        return existing
    content = exact_url(SPDX_LICENSE_URL.format(license_id=license_id))
    identifier = add_notice(notices, content, f"SPDX {license_id}")
    cache[license_id] = identifier
    return identifier


def metadata_notice(
    *, ecosystem: str, name: str, version: str, license_expression: str, source: str
) -> bytes:
    return (
        f"Component: {ecosystem}:{name}@{version}\n"
        f"Declared license: {license_expression}\n"
        f"Source: {source}\n"
    ).encode()


def tar_members(
    content: bytes, label: str
) -> tuple[tarfile.TarFile, list[tarfile.TarInfo]]:
    try:
        # The caller owns the returned archive and closes it in a `finally` block.
        archive = tarfile.open(  # noqa: SIM115
            fileobj=io.BytesIO(content), mode="r:gz"
        )
        members = archive.getmembers()
    except tarfile.TarError as error:
        raise NoticeError(
            f"pinned package is not a valid tar archive: {label}"
        ) from error
    if any(not (member.isfile() or member.isdir()) for member in members):
        archive.close()
        fail(f"pinned package has a non-regular archive member: {label}")
    return archive, members


def cargo_components(
    notices: dict[str, bytes], standard: dict[str, str]
) -> list[dict[str, Any]]:
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    packages = lock.get("package")
    if not isinstance(packages, list):
        fail("Cargo.lock has no package graph")
    metadata = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--locked", "--format-version", "1"],
            cwd=ROOT,
            text=True,
            timeout=120,
        )
    )
    manifests = {
        (package["name"], package["version"], package.get("source")): Path(
            package["manifest_path"]
        )
        for package in metadata["packages"]
    }
    components: list[dict[str, Any]] = []
    for package in packages:
        source = package.get("source")
        if not isinstance(source, str):
            continue
        name = package.get("name")
        version = package.get("version")
        checksum = package.get("checksum")
        if not isinstance(name, str) or not isinstance(version, str):
            fail("Cargo.lock package identity is malformed")
        license_files: list[tuple[str, bytes]] = []
        if source.startswith("registry+"):
            if (
                not isinstance(checksum, str)
                or re.fullmatch(r"[0-9a-f]{64}", checksum) is None
            ):
                fail(f"Cargo registry package has no exact checksum: {name}@{version}")
            encoded_name = urllib.parse.quote(name, safe="")
            encoded_version = urllib.parse.quote(version, safe="")
            url = (
                f"https://static.crates.io/crates/{encoded_name}/"
                f"{encoded_name}-{encoded_version}.crate"
            )
            content = exact_url(url, expected_sha256=checksum)
            archive, members = tar_members(content, f"cargo:{name}@{version}")
            try:
                root = f"{name}-{version}/"
                cargo_toml = archive.extractfile(root + "Cargo.toml")
                if cargo_toml is None:
                    fail(f"Cargo package has no manifest: {name}@{version}")
                manifest = tomllib.loads(cargo_toml.read().decode("utf-8"))
                for member in members:
                    relative = member.name.removeprefix(root)
                    if (
                        member.isfile()
                        and "/" not in relative
                        and relative.lower().startswith(
                            ("license", "copying", "notice")
                        )
                    ):
                        extracted = archive.extractfile(member)
                        if extracted is None:
                            fail(f"cannot read Cargo notice {member.name}")
                        license_files.append((relative, extracted.read()))
            finally:
                archive.close()
        elif source.startswith("git+"):
            manifest_path = manifests.get((name, version, source))
            if manifest_path is None:
                fail(f"Cargo metadata omitted git package {name}@{version}")
            revision = source.rpartition("#")[2]
            checkout = Path(
                subprocess.check_output(
                    [
                        "git",
                        "-C",
                        str(manifest_path.parent),
                        "rev-parse",
                        "--show-toplevel",
                    ],
                    text=True,
                    timeout=30,
                ).strip()
            )
            observed = subprocess.check_output(
                ["git", "-C", str(checkout), "rev-parse", "HEAD"],
                text=True,
                timeout=30,
            ).strip()
            if observed != revision:
                fail(f"Cargo git checkout revision differs: {name}@{version}")
            manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
            for candidate in sorted(checkout.iterdir()):
                if candidate.is_file() and candidate.name.lower().startswith(
                    ("license", "copying", "notice")
                ):
                    license_files.append((candidate.name, candidate.read_bytes()))
        else:
            fail(f"Cargo dependency has unsupported source: {source}")
        package_metadata = manifest.get("package")
        expression = (
            package_metadata.get("license")
            if isinstance(package_metadata, dict)
            else None
        )
        if not isinstance(expression, str) or not expression.strip():
            fail(f"Cargo dependency has no declared license: {name}@{version}")
        identifiers = [
            add_notice(
                notices,
                metadata_notice(
                    ecosystem="cargo",
                    name=name,
                    version=version,
                    license_expression=expression,
                    source=source,
                ),
                f"cargo metadata {name}@{version}",
            )
        ]
        identifiers.extend(
            standard_license_notice(notices, standard, license_id)
            for license_id in license_ids(expression)
        )
        identifiers.extend(
            add_notice(notices, content, f"cargo:{name}@{version}:{relative}")
            for relative, content in sorted(license_files)
        )
        components.append(
            {
                "checksum": checksum if isinstance(checksum, str) else None,
                "declared_license": expression,
                "name": name,
                "notice_ids": sorted(set(identifiers)),
                "source": source,
                "version": version,
            }
        )
    return sorted(
        components, key=lambda item: (item["name"], item["version"], item["source"])
    )


def pnpm_integrities(
    content: bytes, expected: list[tuple[str, str]]
) -> dict[tuple[str, str], str]:
    text = content.decode("utf-8")
    lines = text.splitlines()
    packages_index = lines.index("packages:")
    snapshots_index = lines.index("snapshots:")
    found: dict[tuple[str, str], str] = {}
    index = packages_index + 1
    while index < snapshots_index:
        line = lines[index]
        if not (
            line.startswith("  ") and not line.startswith("    ") and line.endswith(":")
        ):
            index += 1
            continue
        encoded = line[2:-1]
        if encoded.startswith("'") and encoded.endswith("'"):
            locator = encoded[1:-1].replace("''", "'")
        elif encoded.startswith('"') and encoded.endswith('"'):
            locator = json.loads(encoded)
        else:
            locator = encoded
        end = index + 1
        while end < snapshots_index and not (
            lines[end].startswith("  ")
            and not lines[end].startswith("    ")
            and lines[end].endswith(":")
        ):
            end += 1
        matches = re.findall(
            r"integrity:\s*(sha512-[A-Za-z0-9+/]+={0,2})(?:[,} ]|$)",
            "\n".join(lines[index + 1 : end]),
        )
        separator = locator.rfind("@")
        key = (locator[:separator], locator[separator + 1 :])
        if (
            separator <= 0
            or len(matches) != 1
            or INTEGRITY.fullmatch(matches[0]) is None
        ):
            fail(f"pnpm package has no unambiguous integrity: {locator}")
        try:
            digest = base64.b64decode(matches[0].removeprefix("sha512-"), validate=True)
        except ValueError as error:
            raise NoticeError(f"pnpm integrity is malformed: {locator}") from error
        if len(digest) != hashlib.sha512().digest_size:
            fail(f"pnpm integrity has the wrong digest size: {locator}")
        found[key] = matches[0]
        index = end
    if set(found) != set(expected):
        fail("pnpm integrity graph differs from the locked package graph")
    return found


def npm_components(
    notices: dict[str, bytes], standard: dict[str, str], identity
) -> list[dict[str, Any]]:
    lock_content = (ROOT / "pnpm-lock.yaml").read_bytes()
    packages = identity.pnpm_locked_packages(lock_content)
    integrities = pnpm_integrities(lock_content, packages)
    components: list[dict[str, Any]] = []
    for name, version in packages:
        integrity = integrities[(name, version)]
        base_name = name.rsplit("/", maxsplit=1)[-1]
        encoded_name = urllib.parse.quote(name, safe="@")
        url = f"https://registry.npmjs.org/{encoded_name}/-/{base_name}-{version}.tgz"
        content = exact_url(url)
        expected_digest = base64.b64decode(
            integrity.removeprefix("sha512-"), validate=True
        )
        if hashlib.sha512(content).digest() != expected_digest:
            fail(f"pnpm tarball integrity differs: {name}@{version}")
        archive, members = tar_members(content, f"npm:{name}@{version}")
        try:
            manifests = sorted(
                member
                for member in members
                if member.isfile()
                and member.name.endswith("/package.json")
                and member.name.count("/") == 1
            )
            if len(manifests) != 1:
                fail(f"npm package has no unambiguous manifest: {name}@{version}")
            manifest_file = archive.extractfile(manifests[0])
            if manifest_file is None:
                fail(f"cannot read npm package manifest: {name}@{version}")
            metadata = json.loads(manifest_file.read())
            root = manifests[0].name.rpartition("/")[0] + "/"
            license_files: list[tuple[str, bytes]] = []
            for member in members:
                relative = member.name.removeprefix(root)
                if (
                    member.isfile()
                    and member.name.startswith(root)
                    and "/" not in relative
                    and relative.lower().startswith(("license", "copying", "notice"))
                ):
                    extracted = archive.extractfile(member)
                    if extracted is None:
                        fail(f"cannot read npm notice {member.name}")
                    license_files.append((relative, extracted.read()))
        finally:
            archive.close()
        expression = metadata.get("license") if isinstance(metadata, dict) else None
        if not isinstance(expression, str) or not expression.strip():
            fail(f"npm dependency has no declared license: {name}@{version}")
        identifiers = [
            add_notice(
                notices,
                metadata_notice(
                    ecosystem="npm",
                    name=name,
                    version=version,
                    license_expression=expression,
                    source=url,
                ),
                f"npm metadata {name}@{version}",
            )
        ]
        identifiers.extend(
            standard_license_notice(notices, standard, license_id)
            for license_id in license_ids(expression)
        )
        identifiers.extend(
            add_notice(notices, notice, f"npm:{name}@{version}:{relative}")
            for relative, notice in sorted(license_files)
        )
        components.append(
            {
                "declared_license": expression,
                "integrity": integrity,
                "name": name,
                "notice_ids": sorted(set(identifiers)),
                "version": version,
            }
        )
    return sorted(components, key=lambda item: (item["name"], item["version"]))


def oci_base_components(
    image: str, notices: dict[str, bytes], standard: dict[str, str]
) -> dict[str, Any]:
    try:
        installed_database = subprocess.check_output(
            [
                "docker",
                "run",
                "--rm",
                "--network",
                "none",
                "--platform",
                "linux/amd64",
                "--entrypoint",
                "/bin/sh",
                image,
                "-eu",
                "-c",
                "cat /lib/apk/db/installed",
            ],
            timeout=120,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise NoticeError(
            "cannot observe the pinned OCI base package database"
        ) from error
    packages: list[dict[str, Any]] = []
    try:
        records = installed_database.decode("utf-8").strip().split("\n\n")
    except UnicodeError as error:
        raise NoticeError("pinned OCI base package database is not UTF-8") from error
    for record in records:
        fields = {
            line.partition(":")[0]: line.partition(":")[2]
            for line in record.splitlines()
            if ":" in line
        }
        required = [fields.get(key) for key in ("P", "V", "A", "L", "U")]
        if not all(isinstance(value, str) and value for value in required):
            fail("pinned OCI base package database is malformed")
        name, version, architecture, expression, source = required
        identifiers = [
            add_notice(
                notices,
                metadata_notice(
                    ecosystem="apk",
                    name=name,
                    version=version,
                    license_expression=expression,
                    source=source,
                ),
                f"apk metadata {name}@{version}",
            )
        ]
        identifiers.extend(
            standard_license_notice(notices, standard, license_id)
            for license_id in license_ids(expression)
        )
        packages.append(
            {
                "architecture": architecture,
                "declared_license": expression,
                "name": name,
                "notice_ids": sorted(set(identifiers)),
                "source": source,
                "version": version,
            }
        )
    if not packages:
        fail("pinned OCI base has no package inventory")
    return {
        "image": image,
        "installed_database_sha256": sha256(installed_database),
        "packages": sorted(packages, key=lambda item: (item["name"], item["version"])),
    }


def render_notices(sections: dict[str, bytes]) -> bytes:
    output = bytearray(
        b"WorldStream Third-Party Notices\n"
        b"\n"
        b"This deterministic bundle preserves upstream declared-license metadata, "
        b"license terms, and notices for the exact locked Cargo, pnpm, and pinned "
        b"OCI-base component inventories. Component-to-section bindings are in "
        b"THIRD-PARTY-NOTICES.json.\n"
        b"\n"
    )
    for identifier, content in sorted(sections.items()):
        output.extend(f"===== BEGIN NOTICE {identifier} =====\n".encode())
        output.extend(content)
        output.extend(f"===== END NOTICE {identifier} =====\n\n".encode())
    return bytes(output).rstrip(b"\n") + b"\n"


def write_atomic(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def generate() -> None:
    identity = identity_module()
    source_entries = identity.source_entries_from_root(ROOT)
    image = identity.expected_base_image(source_entries)
    sections: dict[str, bytes] = {}
    standard: dict[str, str] = {}
    components = {
        "cargo": cargo_components(sections, standard),
        "npm": npm_components(sections, standard, identity),
        "oci_base": oci_base_components(image, sections, standard),
    }
    notices = render_notices(sections)
    manifest = {
        "components": components,
        "inputs": {
            "Cargo.lock": sha256((ROOT / "Cargo.lock").read_bytes()),
            "packaging/oci/base-image.txt": sha256(
                (ROOT / "packaging/oci/base-image.txt").read_bytes()
            ),
            "pnpm-lock.yaml": sha256((ROOT / "pnpm-lock.yaml").read_bytes()),
            "spdx_license_list_revision": SPDX_LICENSE_LIST_REVISION,
        },
        "license_texts": dict(sorted(standard.items())),
        "notices": {
            "path": "licenses/THIRD-PARTY-NOTICES.txt",
            "sha256": sha256(notices),
            "size_bytes": len(notices),
        },
        "schema": NOTICE_SCHEMA,
    }
    write_atomic(NOTICES_PATH, notices)
    write_atomic(MANIFEST_PATH, canonical_json(manifest))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--write",
        action="store_true",
        help="replace the checked-in bundle after every exact input is verified",
    )
    args = parser.parse_args()
    if not args.write:
        fail("generation is explicit; pass --write")
    generate()
    print("wrote deterministic third-party notice bundle")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except NoticeError as error:
        print(f"third-party notices: FAIL: {error}", file=sys.stderr)
        raise SystemExit(1) from error

#!/usr/bin/env python3
"""Build and verify the standalone native Agent Swarm distribution.

This intentionally lives outside ``scripts/package.py``.  The frozen
WorldStream server release inventory does not advertise macOS, while Agent
Swarm is a separate local application with its own native support contract.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import stat
import tarfile
import tempfile
import zipfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath

SCHEMA = "worldstream/agent-swarm-install/v1"
PACK_ID = "worldstream.agent-swarm"
DIGEST = re.compile(r"blake3:[0-9a-f]{64}\Z")
MAX_FILE_BYTES = 2 * 1024 * 1024 * 1024
MAX_ARCHIVE_MEMBERS = 128
LOCAL_CONFIG_PATH = "config/local.toml"
DEFAULT_LOCAL_CONFIG = (
    Path(__file__).resolve().parents[1] / "packaging/agent-swarm/local.toml"
)


class PackageError(RuntimeError):
    """A fail-closed package or verification error."""


@dataclass(frozen=True)
class Target:
    name: str
    system: str
    machines: tuple[str, ...]
    suffix: str
    executable_suffix: str


TARGETS = {
    "macos-arm64": Target("macos-arm64", "Darwin", ("arm64", "aarch64"), ".tar.gz", ""),
    "macos-x86_64": Target(
        "macos-x86_64", "Darwin", ("x86_64", "amd64"), ".tar.gz", ""
    ),
    "windows-x64": Target(
        "windows-x64", "Windows", ("amd64", "x86_64"), ".zip", ".exe"
    ),
}

BINARIES = (
    "worldstream-agent-swarm",
    "worldstream-agent-swarm-process-guard",
    "worldstream-agent-swarm-controlled-worker",
    "worldstream-agent-swarmd",
    "worldstream-agent-swarmctl",
    "worldstreamd",
    "worldstreamctl",
    "worldstream-studio-supervisor",
    "worldstream-assignment-mcp",
    "worldstream-managed-agent-host",
)


def _regular_file(path: Path, label: str) -> None:
    try:
        info = path.lstat()
    except OSError as error:
        raise PackageError(f"missing {label}: {path}") from error
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
        raise PackageError(f"{label} must be a regular non-symlink file: {path}")
    if info.st_size <= 0 or info.st_size > MAX_FILE_BYTES:
        raise PackageError(f"{label} has an invalid size: {path}")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _canonical_json(value: object) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def _pack_descriptor(pack: Path) -> dict[str, object]:
    _regular_file(pack, "Activity Pack")
    try:
        with tarfile.open(pack, "r:") as archive:
            members = archive.getmembers()
            if len(members) > MAX_ARCHIVE_MEMBERS:
                raise PackageError("Activity Pack contains too many members")
            for member in members:
                path = PurePosixPath(member.name)
                if (
                    path.is_absolute()
                    or ".." in path.parts
                    or member.issym()
                    or member.islnk()
                ):
                    raise PackageError("Activity Pack contains an unsafe member")
            member = archive.getmember("descriptor.json")
            source = archive.extractfile(member)
            if source is None or member.size > 4 * 1024 * 1024:
                raise PackageError("Activity Pack descriptor is unavailable")
            value = json.loads(source.read())
    except (tarfile.TarError, KeyError, json.JSONDecodeError, OSError) as error:
        raise PackageError(
            "Activity Pack is not a valid bounded WorldStream bundle"
        ) from error
    if not isinstance(value, dict):
        raise PackageError("Activity Pack descriptor must be an object")
    return value


def _validate_pack(pack: Path, version: str, digest: str) -> None:
    if not DIGEST.fullmatch(digest):
        raise PackageError("pack digest must be a lowercase blake3 reference")
    descriptor = _pack_descriptor(pack)
    if descriptor.get("pack_id") != PACK_ID:
        raise PackageError("Activity Pack identifier mismatch")
    if descriptor.get("explanatory_version") != version:
        raise PackageError("Activity Pack version mismatch")
    if descriptor.get("revision_digest") != digest:
        raise PackageError("Activity Pack semantic revision mismatch")


def inspect_pack(pack: Path) -> dict[str, str]:
    """Return the exact package identity after validating its descriptor."""

    descriptor = _pack_descriptor(pack)
    pack_id = descriptor.get("pack_id")
    version = descriptor.get("explanatory_version")
    digest = descriptor.get("revision_digest")
    if (
        pack_id != PACK_ID
        or not isinstance(version, str)
        or not version
        or not isinstance(digest, str)
        or not DIGEST.fullmatch(digest)
    ):
        raise PackageError("Activity Pack descriptor identity is invalid")
    return {"id": pack_id, "version": version, "digest": digest}


def _validate_native(target: Target) -> None:
    system = platform.system()
    machine = platform.machine().lower()
    if system != target.system or machine not in target.machines:
        raise PackageError(
            f"target {target.name} must be packaged natively; found {system}/{machine}"
        )


def _source_epoch() -> int:
    raw = os.environ.get("SOURCE_DATE_EPOCH", "0")
    try:
        value = int(raw)
    except ValueError as error:
        raise PackageError(
            "SOURCE_DATE_EPOCH must be a non-negative integer"
        ) from error
    if value < 0:
        raise PackageError("SOURCE_DATE_EPOCH must be a non-negative integer")
    # ZIP cannot represent dates before 1980.
    return value


def _collect_payload(
    stage: Path,
    target: Target,
    binary_dir: Path,
    pack: Path,
    version: str,
    pack_version: str,
    pack_digest: str,
    configuration: Path,
) -> dict[str, object]:
    root = stage / f"worldstream-agent-swarm-{version}-{target.name}"
    bin_dir = root / "bin"
    pack_dir = root / "packs" / PACK_ID / pack_digest.removeprefix("blake3:")
    metadata_dir = root / "metadata"
    config_dir = root / "config"
    bin_dir.mkdir(parents=True)
    pack_dir.mkdir(parents=True)
    metadata_dir.mkdir(parents=True)
    config_dir.mkdir(parents=True)

    files: dict[str, str] = {}
    for stem in BINARIES:
        name = stem + target.executable_suffix
        source = binary_dir / name
        _regular_file(source, f"binary {name}")
        if target.system != "Windows" and not source.stat().st_mode & stat.S_IXUSR:
            raise PackageError(f"binary is not executable: {source}")
        destination = bin_dir / name
        destination.write_bytes(source.read_bytes())
        destination.chmod(0o755)
        files[destination.relative_to(root).as_posix()] = _sha256(destination)

    pack_name = f"{PACK_ID}-{pack_version}-{pack_digest[7:19]}.wspack"
    installed_pack = pack_dir / pack_name
    installed_pack.write_bytes(pack.read_bytes())
    installed_pack.chmod(0o644)
    files[installed_pack.relative_to(root).as_posix()] = _sha256(installed_pack)

    _regular_file(configuration, "local application configuration")
    installed_configuration = root / LOCAL_CONFIG_PATH
    installed_configuration.write_bytes(configuration.read_bytes())
    installed_configuration.chmod(0o644)
    files[LOCAL_CONFIG_PATH] = _sha256(installed_configuration)

    install = {
        "schema": SCHEMA,
        "application_version": version,
        "target": target.name,
        "execution_default": "suspended_until_explicit_resume",
        "provider_management": "discover_only_never_install_or_reconfigure",
        "state_location": "platform_user_data/worldstream/agent-swarm",
        "configuration": {
            "path": LOCAL_CONFIG_PATH,
            "state_roots": "operator_selected_protected_external_paths",
        },
        "pack": {
            "id": PACK_ID,
            "version": pack_version,
            "digest": pack_digest,
            "path": installed_pack.relative_to(root).as_posix(),
        },
        "files": dict(sorted(files.items())),
    }
    install_path = metadata_dir / "install.json"
    install_path.write_bytes(_canonical_json(install))
    install_path.chmod(0o644)
    files[install_path.relative_to(root).as_posix()] = _sha256(install_path)

    checksums = root / "checksums.sha256"
    checksums.write_text(
        "".join(f"{digest}  {name}\n" for name, digest in sorted(files.items())),
        encoding="utf-8",
        newline="\n",
    )
    checksums.chmod(0o644)
    return {"root": root.name, "install": install, "checksums": files}


def _write_tar(stage: Path, root_name: str, output: Path, epoch: int) -> None:
    root = stage / root_name
    with tarfile.open(output, "w:gz", format=tarfile.PAX_FORMAT) as archive:
        for path in [root, *sorted(root.rglob("*"), key=lambda item: item.as_posix())]:
            relative = path.relative_to(stage).as_posix()
            info = archive.gettarinfo(str(path), relative)
            info.uid = 0
            info.gid = 0
            info.uname = ""
            info.gname = ""
            info.mtime = epoch
            if info.isdir():
                info.mode = 0o755
                archive.addfile(info)
            else:
                info.mode = 0o755 if relative.startswith(f"{root_name}/bin/") else 0o644
                with path.open("rb") as source:
                    archive.addfile(info, source)


def _write_zip(stage: Path, root_name: str, output: Path, epoch: int) -> None:
    import datetime

    root = stage / root_name
    date = datetime.datetime.fromtimestamp(max(epoch, 315532800), datetime.timezone.utc)
    date_time = (date.year, date.month, date.day, date.hour, date.minute, date.second)
    with zipfile.ZipFile(
        output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as archive:
        for path in sorted(root.rglob("*"), key=lambda item: item.as_posix()):
            if path.is_dir():
                continue
            name = path.relative_to(stage).as_posix()
            info = zipfile.ZipInfo(name, date_time)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (0o755 if "/bin/" in name else 0o644) << 16
            archive.writestr(info, path.read_bytes())


def build(
    *,
    target_name: str,
    binary_dir: Path,
    pack: Path,
    output_dir: Path,
    version: str,
    pack_version: str,
    pack_digest: str,
    configuration: Path = DEFAULT_LOCAL_CONFIG,
    enforce_native: bool = True,
) -> Path:
    target = TARGETS[target_name]
    if enforce_native:
        _validate_native(target)
    _validate_pack(pack, pack_version, pack_digest)
    output_dir.mkdir(parents=True, exist_ok=True)
    output = (
        output_dir / f"worldstream-agent-swarm-{version}-{target.name}{target.suffix}"
    )
    if output.exists():
        raise PackageError(f"refusing to overwrite existing archive: {output}")
    with tempfile.TemporaryDirectory(prefix="agent-swarm-package-") as temporary:
        stage = Path(temporary)
        payload = _collect_payload(
            stage,
            target,
            binary_dir,
            pack,
            version,
            pack_version,
            pack_digest,
            configuration,
        )
        if target.suffix == ".zip":
            _write_zip(stage, str(payload["root"]), output, _source_epoch())
        else:
            _write_tar(stage, str(payload["root"]), output, _source_epoch())
    verify(output, target_name=target_name)
    return output


def _safe_member(name: str) -> PurePosixPath:
    path = PurePosixPath(name)
    if path.is_absolute() or not path.parts or ".." in path.parts or "\\" in name:
        raise PackageError(f"unsafe archive member: {name}")
    return path


def _read_archive(archive_path: Path) -> dict[str, tuple[bytes, int]]:
    _regular_file(archive_path, "archive")
    files: dict[str, tuple[bytes, int]] = {}
    if archive_path.name.endswith(".zip"):
        try:
            with zipfile.ZipFile(archive_path) as archive:
                infos = archive.infolist()
                if len(infos) > MAX_ARCHIVE_MEMBERS:
                    raise PackageError("archive contains too many members")
                for info in infos:
                    path = _safe_member(info.filename)
                    if info.is_dir():
                        continue
                    if info.file_size <= 0 or info.file_size > MAX_FILE_BYTES:
                        raise PackageError(f"invalid archive member size: {path}")
                    mode = info.external_attr >> 16
                    if stat.S_ISLNK(mode):
                        raise PackageError(f"archive contains a link: {path}")
                    files[path.as_posix()] = (archive.read(info), mode)
        except (zipfile.BadZipFile, OSError) as error:
            raise PackageError("invalid ZIP archive") from error
    else:
        try:
            with tarfile.open(archive_path, "r:gz") as archive:
                members = archive.getmembers()
                if len(members) > MAX_ARCHIVE_MEMBERS:
                    raise PackageError("archive contains too many members")
                for member in members:
                    path = _safe_member(member.name)
                    if member.isdir():
                        continue
                    if (
                        not member.isfile()
                        or member.size <= 0
                        or member.size > MAX_FILE_BYTES
                    ):
                        raise PackageError(f"invalid archive member: {path}")
                    source = archive.extractfile(member)
                    if source is None:
                        raise PackageError(f"unreadable archive member: {path}")
                    files[path.as_posix()] = (source.read(), member.mode)
        except (tarfile.TarError, OSError, EOFError) as error:
            raise PackageError("invalid tar archive") from error
    return files


def verify(
    archive_path: Path, *, target_name: str, require_configuration: bool = True
) -> dict[str, object]:
    install, _ = _read_verified_archive(
        archive_path,
        target_name=target_name,
        require_configuration=require_configuration,
    )
    return install


def _read_verified_archive(
    archive_path: Path, *, target_name: str, require_configuration: bool = True
) -> tuple[dict[str, object], dict[str, tuple[bytes, int]]]:
    files = _read_archive(archive_path)
    install = _verify_archive_files(
        files,
        target=TARGETS[target_name],
        require_configuration=require_configuration,
    )
    return install, files


def _verify_archive_files(
    files: dict[str, tuple[bytes, int]],
    *,
    target: Target,
    require_configuration: bool,
) -> dict[str, object]:
    roots = {PurePosixPath(name).parts[0] for name in files}
    if len(roots) != 1:
        raise PackageError("archive must contain exactly one root directory")
    root = next(iter(roots))
    install_name = f"{root}/metadata/install.json"
    checksums_name = f"{root}/checksums.sha256"
    try:
        install = json.loads(files[install_name][0])
        checksum_text = files[checksums_name][0].decode("utf-8")
    except (KeyError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PackageError("archive metadata is missing or invalid") from error
    if not isinstance(install, dict) or install.get("schema") != SCHEMA:
        raise PackageError("archive install schema mismatch")
    if install.get("target") != target.name:
        raise PackageError("archive target mismatch")
    if install.get("execution_default") != "suspended_until_explicit_resume":
        raise PackageError("archive would automatically resume execution")
    if (
        install.get("provider_management")
        != "discover_only_never_install_or_reconfigure"
    ):
        raise PackageError("archive provider-management contract mismatch")
    pack = install.get("pack")
    if (
        not isinstance(pack, dict)
        or pack.get("id") != PACK_ID
        or not DIGEST.fullmatch(str(pack.get("digest", "")))
    ):
        raise PackageError("archive Pack metadata mismatch")
    configuration = install.get("configuration")
    if (configuration is not None or require_configuration) and (
        not isinstance(configuration, dict)
        or configuration.get("path") != LOCAL_CONFIG_PATH
        or configuration.get("state_roots")
        != "operator_selected_protected_external_paths"
    ):
        raise PackageError("archive local configuration metadata mismatch")
    declared = install.get("files")
    if not isinstance(declared, dict) or not declared:
        raise PackageError("archive file inventory is missing")
    expected_checksums: list[tuple[str, str]] = []
    for relative, expected_digest in sorted(declared.items()):
        if not isinstance(relative, str) or not isinstance(expected_digest, str):
            raise PackageError("archive file inventory is invalid")
        full = f"{root}/{relative}"
        if full not in files:
            raise PackageError(f"archive inventory member is missing: {relative}")
        actual = hashlib.sha256(files[full][0]).hexdigest()
        if actual != expected_digest:
            raise PackageError(f"archive inventory digest mismatch: {relative}")
        expected_checksums.append((relative, expected_digest))
    expected_checksums.append(
        ("metadata/install.json", hashlib.sha256(files[install_name][0]).hexdigest())
    )
    expected_text = "".join(
        f"{digest}  {name}\n" for name, digest in sorted(expected_checksums)
    )
    if checksum_text != expected_text:
        raise PackageError("checksums file does not exactly match install inventory")
    expected_members = {
        install_name,
        checksums_name,
        *(f"{root}/{relative}" for relative in declared),
    }
    if set(files) != expected_members:
        raise PackageError("archive contains files outside the exact install inventory")
    for stem in BINARIES:
        name = f"{root}/bin/{stem}{target.executable_suffix}"
        if name not in files:
            raise PackageError(f"required binary is missing: {name}")
        if target.system != "Windows" and not files[name][1] & stat.S_IXUSR:
            raise PackageError(f"required binary is not executable: {name}")
    pack_path = pack.get("path")
    if not isinstance(pack_path, str) or f"{root}/{pack_path}" not in files:
        raise PackageError("retained exact Activity Pack is missing")
    if configuration is not None:
        configuration_path = configuration.get("path")
        if (
            not isinstance(configuration_path, str)
            or f"{root}/{configuration_path}" not in files
        ):
            raise PackageError("retained local application configuration is missing")
    return install


def extract_verified(
    archive_path: Path, *, target_name: str, output_dir: Path
) -> Path:
    """Verify one archive, then extract its bounded regular-file inventory."""

    _, files = _read_verified_archive(
        archive_path,
        target_name=target_name,
    )
    roots = {PurePosixPath(name).parts[0] for name in files}
    if len(roots) != 1:  # Kept local so this helper never trusts verify internals.
        raise PackageError("archive must contain exactly one root directory")
    try:
        output_dir.mkdir(parents=True)
        for name, (content, mode) in files.items():
            destination = output_dir.joinpath(*PurePosixPath(name).parts)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
            destination.chmod(0o755 if mode & stat.S_IXUSR else 0o644)
    except FileExistsError as error:
        raise PackageError(
            f"refusing to replace extraction directory: {output_dir}"
        ) from error
    except OSError as error:
        raise PackageError(f"could not extract verified archive: {output_dir}") from error
    return output_dir / next(iter(roots))


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_command = commands.add_parser("build")
    build_command.add_argument("--target", choices=sorted(TARGETS), required=True)
    build_command.add_argument("--binary-dir", type=Path, required=True)
    build_command.add_argument("--pack", type=Path, required=True)
    build_command.add_argument("--output", type=Path, required=True)
    build_command.add_argument("--version", required=True)
    build_command.add_argument("--pack-version", required=True)
    build_command.add_argument("--pack-digest", required=True)
    build_command.add_argument("--config", type=Path, default=DEFAULT_LOCAL_CONFIG)
    verify_command = commands.add_parser("verify")
    verify_command.add_argument("archive", type=Path)
    verify_command.add_argument("--target", choices=sorted(TARGETS), required=True)
    extract_command = commands.add_parser("extract")
    extract_command.add_argument("archive", type=Path)
    extract_command.add_argument("--target", choices=sorted(TARGETS), required=True)
    extract_command.add_argument("--output", type=Path, required=True)
    inspect_command = commands.add_parser("inspect-pack")
    inspect_command.add_argument("pack", type=Path)
    return parser


def main() -> int:
    args = _parser().parse_args()
    try:
        if args.command == "build":
            archive = build(
                target_name=args.target,
                binary_dir=args.binary_dir,
                pack=args.pack,
                output_dir=args.output,
                version=args.version,
                pack_version=args.pack_version,
                pack_digest=args.pack_digest,
                configuration=args.config,
            )
            print(archive)
        elif args.command == "verify":
            print(
                json.dumps(
                    verify(args.archive, target_name=args.target), sort_keys=True
                )
            )
        elif args.command == "extract":
            print(
                extract_verified(
                    args.archive,
                    target_name=args.target,
                    output_dir=args.output,
                )
            )
        else:
            print(json.dumps(inspect_pack(args.pack), sort_keys=True))
    except PackageError as error:
        print(f"agent-swarm package error: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

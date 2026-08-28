#!/usr/bin/env python3
"""Install the owner-controlled deterministic Counter managed-host fixture.

The installer copies one reviewed host executable to a digest-addressed,
owner-only location and publishes only immutable owner manifests. It does not
create a Studio Agent Profile or place a provider credential in a command line,
environment variable, manifest, or diagnostic.
"""

from __future__ import annotations

import argparse
import json
import os
import secrets
import stat
import sys
from pathlib import Path
from typing import Any

import blake3

COUNTER_PACK_ID = "worldstream.counter"
COUNTER_PACK_REVISION = "4.0.0"
COUNTER_PACK_DIGEST = (
    "blake3:2a1d2e493cbaffa3803724dfef42d35c167db2237aa9b1e113dfb79679e9c052"
)
MANIFEST_NAME = "counter-managed-reference-v1.json"
PROVIDER_CONFIG_NAME = "managed-counter-provider.json"
TEMPLATE_ID = "counter-managed-reference"
TEMPLATE_REVISION = "v1"
INSTANCE_ID = "counter-managed-reference-01"
MAX_HOST_BYTES = 256 * 1024 * 1024
MODEL_CREDENTIAL_ID = "local-openai"
MODEL_CREDENTIAL_MANIFEST = f"{MODEL_CREDENTIAL_ID}.json"


def _owner_only_directory(path: Path) -> Path:
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ValueError("fixture storage directory is unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise ValueError("fixture storage directory must be a directory")
    if metadata.st_mode & 0o077:
        raise ValueError("fixture storage directory must be owner-only")
    if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
        raise ValueError("fixture storage directory has the wrong owner")
    return path.resolve()


def _regular_source(path: Path) -> Path:
    absolute = Path(os.path.abspath(path))
    try:
        metadata = absolute.lstat()
    except OSError as error:
        raise ValueError("approved managed host executable is unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ValueError("approved managed host executable must be a regular file")
    if metadata.st_size == 0:
        raise ValueError("approved managed host executable is empty")
    return absolute


def _source_bytes(path: Path) -> tuple[bytes, str]:
    try:
        content = path.read_bytes()
    except OSError as error:
        raise ValueError("approved managed host executable is unavailable") from error
    if not content or len(content) > MAX_HOST_BYTES:
        raise ValueError(
            "approved managed host executable is outside the accepted size"
        )
    return content, blake3.blake3(content).hexdigest()


def _owner_only_secret_file(path: Path) -> bytes:
    absolute = Path(os.path.abspath(path))
    try:
        metadata = absolute.lstat()
    except OSError as error:
        raise ValueError("model credential file is unavailable") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ValueError("model credential file must be a regular file")
    if metadata.st_mode & 0o077:
        raise ValueError("model credential file must be owner-only")
    if hasattr(os, "getuid") and metadata.st_uid != os.getuid():
        raise ValueError("model credential file has the wrong owner")
    try:
        credential = absolute.read_bytes()
    except OSError as error:
        raise ValueError("model credential file is unavailable") from error
    if (
        not credential
        or len(credential) > 16 * 1024
        or b"\r" in credential
        or b"\n" in credential
    ):
        raise ValueError("model credential material is invalid")
    return credential


def _fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def _matches_immutable_target(target: Path, content: bytes, mode: int) -> bool:
    try:
        metadata = target.lstat()
        existing = target.read_bytes()
    except OSError as error:
        raise ValueError("fixture immutable record is unavailable") from error
    return (
        not stat.S_ISLNK(metadata.st_mode)
        and stat.S_ISREG(metadata.st_mode)
        and stat.S_IMODE(metadata.st_mode) == mode
        and existing == content
    )


def _publish_immutable_bytes(target: Path, content: bytes, mode: int) -> None:
    """Publish `content` as an absent-or-complete immutable target."""

    if target.exists():
        if not _matches_immutable_target(target, content, mode):
            raise ValueError(
                "fixture immutable record conflicts with the approved install"
            )
        return

    temporary = target.with_name(f".{target.name}.{secrets.token_hex(12)}.tmp")
    descriptor: int | None = None
    try:
        descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
        with os.fdopen(descriptor, "wb", closefd=False) as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, mode)
        try:
            os.link(temporary, target)
        except FileExistsError:
            if not _matches_immutable_target(target, content, mode):
                raise ValueError(
                    "fixture immutable record conflicts with the approved install"
                )
        _fsync_directory(target.parent)
    except OSError as error:
        raise ValueError("fixture immutable record could not be published") from error
    finally:
        if descriptor is not None:
            os.close(descriptor)
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass


def _immutable_copy(source: Path, install_root: Path) -> tuple[Path, str]:
    content, digest = _source_bytes(source)
    directory = _owner_only_directory(install_root / "managed-host" / digest)
    target = directory / source.name
    _publish_immutable_bytes(target, content, 0o700)
    if blake3.blake3(target.read_bytes()).hexdigest() != digest:
        raise ValueError("approved managed host executable copy is invalid")
    return target, digest


def _encoded(value: dict[str, Any]) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode(
        "utf-8"
    )


def _manifest(host: Path, digest: str) -> dict[str, Any]:
    return {
        "schema": "worldstream/runner-template/v1",
        "template_id": TEMPLATE_ID,
        "revision": TEMPLATE_REVISION,
        "display_name": "Counter deterministic managed reference",
        "executable": {"path": str(host), "blake3": digest},
        "compatibility": [
            {
                "activity_pack_id": COUNTER_PACK_ID,
                "exact_revisions": [COUNTER_PACK_REVISION],
            }
        ],
        "capacity": {"maximum_concurrent_invocations": 1},
        "health": {"path": "/healthz", "timeout_ms": 250, "stale_after_ms": 5000},
        "non_secret_environment": {},
        "secret_environment": [],
        "instances": [
            {"instance_id": INSTANCE_ID, "health_address": "127.0.0.1:19432"}
        ],
    }


def _provider_configuration(port: int) -> dict[str, Any]:
    if not 1 <= port <= 65535:
        raise ValueError("provider configuration port is outside the accepted range")
    return {
        "schema": "worldstream/counter-deterministic-provider/v1",
        "bind_address": "127.0.0.1",
        "port": port,
        "model_id": "counter-deterministic",
        "counter_pack_digest": COUNTER_PACK_DIGEST,
    }


def _credential_manifest(reference: str) -> dict[str, Any]:
    return {
        "schema": "worldstream/model-provider-credential/v1",
        "credential_id": MODEL_CREDENTIAL_ID,
        "display_name": "Counter deterministic loopback",
        "provider": "open_ai_compatible",
        "secret": {"kind": "model_provider", "reference": reference},
    }


def _reference_from_manifest(path: Path) -> str | None:
    if not path.exists():
        return None
    try:
        metadata = path.lstat()
        value = json.loads(path.read_text("utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("model provider credential manifest is invalid") from error
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or stat.S_IMODE(metadata.st_mode) != 0o600
        or not isinstance(value, dict)
        or value.get("schema") != "worldstream/model-provider-credential/v1"
        or value.get("credential_id") != MODEL_CREDENTIAL_ID
        or value.get("provider") != "open_ai_compatible"
        or not isinstance(value.get("secret"), dict)
        or value["secret"].get("kind") != "model_provider"
        or not isinstance(value["secret"].get("reference"), str)
    ):
        raise ValueError("model provider credential manifest is invalid")
    reference = value["secret"]["reference"]
    if len(reference) != 64 or any(
        byte not in "0123456789abcdef" for byte in reference
    ):
        raise ValueError("model provider credential manifest is invalid")
    return reference


def _matching_retained_model_reference(vault: Path, credential: bytes) -> str | None:
    matches: list[str] = []
    try:
        entries = list(vault.iterdir())
    except OSError as error:
        raise ValueError("protected model credential storage is unavailable") from error
    for path in entries:
        name = path.name
        prefix = "model-provider-"
        if not name.startswith(prefix) or not name.endswith(".secret"):
            continue
        reference = name.removeprefix(prefix).removesuffix(".secret")
        if len(reference) != 64 or any(
            byte not in "0123456789abcdef" for byte in reference
        ):
            raise ValueError("protected model credential storage is invalid")
        try:
            metadata = path.lstat()
            if (
                stat.S_ISLNK(metadata.st_mode)
                or not stat.S_ISREG(metadata.st_mode)
                or stat.S_IMODE(metadata.st_mode) != 0o600
                or metadata.st_size > 16 * 1024
            ):
                raise ValueError("protected model credential storage is invalid")
            retained = path.read_bytes()
        except OSError as error:
            raise ValueError(
                "protected model credential storage is unavailable"
            ) from error
        # A protected vault may contain credentials for other named providers.
        # They are not a conflict with this exact fixture credential and must
        # not prevent local installation.  Only equal material can be reused.
        if retained == credential:
            matches.append(reference)
    if len(matches) > 1:
        raise ValueError(
            "protected model credential storage has ambiguous retained material"
        )
    return matches[0] if matches else None


def _install_model_credential(
    credential_file: Path, supervisor_state_dir: Path, credentials_dir: Path
) -> None:
    credential = _owner_only_secret_file(credential_file)
    credentials = _owner_only_directory(credentials_dir)
    manifest_path = credentials / MODEL_CREDENTIAL_MANIFEST
    retained_reference = _reference_from_manifest(manifest_path)
    # Create the state root itself before its protected child.  ``mkdir`` only
    # applies its requested mode to the leaf, so relying on ``secrets`` alone
    # could leave a newly-created parent at the platform default mode.
    state_root = _owner_only_directory(supervisor_state_dir)
    vault = _owner_only_directory(state_root / "secrets")
    if retained_reference is None:
        retained_reference = _matching_retained_model_reference(vault, credential)
        if retained_reference is None:
            for _ in range(16):
                candidate = secrets.token_hex(32)
                target = vault / f"model-provider-{candidate}.secret"
                try:
                    _publish_immutable_bytes(target, credential, 0o600)
                except ValueError:
                    continue
                retained_reference = candidate
                break
            if retained_reference is None:
                raise ValueError("protected model credential storage is unavailable")
    secret_path = vault / f"model-provider-{retained_reference}.secret"
    if not _matches_immutable_target(secret_path, credential, 0o600):
        raise ValueError("model provider credential conflicts with retained material")
    _publish_immutable_bytes(
        manifest_path, _encoded(_credential_manifest(retained_reference)), 0o600
    )


def install_fixture(
    source_executable: Path,
    install_root: Path,
    runner_templates_dir: Path,
    provider_port: int,
    *,
    credential_file: Path | None = None,
    supervisor_state_dir: Path | None = None,
    model_provider_credentials_dir: Path | None = None,
) -> None:
    """Install the reviewed host and immutable Counter v4 fixture records."""

    source = _regular_source(source_executable)
    root = _owner_only_directory(install_root)
    try:
        _publish_immutable_bytes(
            root / PROVIDER_CONFIG_NAME,
            _encoded(_provider_configuration(provider_port)),
            0o600,
        )
    except ValueError as error:
        raise ValueError(
            "provider configuration conflicts with the installed fixture"
        ) from error
    host, digest = _immutable_copy(source, root)

    manifests = _owner_only_directory(runner_templates_dir)
    _publish_immutable_bytes(
        manifests / MANIFEST_NAME, _encoded(_manifest(host, digest)), 0o600
    )
    credential_arguments = (
        credential_file,
        supervisor_state_dir,
        model_provider_credentials_dir,
    )
    if any(value is not None for value in credential_arguments):
        if any(value is None for value in credential_arguments):
            raise ValueError(
                "model credential installation requires all protected file paths"
            )
        _install_model_credential(
            credential_file,
            supervisor_state_dir,
            model_provider_credentials_dir,
        )


def main(arguments: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-executable", required=True, type=Path)
    parser.add_argument("--install-root", required=True, type=Path)
    parser.add_argument("--runner-templates-dir", required=True, type=Path)
    parser.add_argument("--provider-port", required=True, type=int)
    parser.add_argument("--credential-file", type=Path)
    parser.add_argument("--supervisor-state-dir", type=Path)
    parser.add_argument("--model-provider-credentials-dir", type=Path)
    args = parser.parse_args(arguments)
    try:
        install_fixture(
            args.source_executable,
            args.install_root,
            args.runner_templates_dir,
            args.provider_port,
            credential_file=args.credential_file,
            supervisor_state_dir=args.supervisor_state_dir,
            model_provider_credentials_dir=args.model_provider_credentials_dir,
        )
    except ValueError as error:
        print(str(error), file=sys.stderr)
        return 2
    print("deterministic Counter managed-host fixture installed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Emit strict platform diagnostics from owning typed reports and artifacts."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import re
import stat
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import tomllib

ROOT = Path(__file__).resolve().parents[1]
SCHEMA = "worldstream/release-platform-diagnostic/v1"
GATE_SCHEMA = "worldstream/compatibility-gate-report/v1"
MANIFEST = ROOT / "compatibility.toml"
PACKAGE_SCRIPT = ROOT / "scripts/package.py"
OCI_LAYOUT_VERIFIER = ROOT / "scripts/verify-oci-layout.py"
BUILD_IDENTITY_PATH = ROOT / "scripts/release_build_identity.py"
BLAKE3_IMPLEMENTATION_PATH = ROOT / "scripts/manifest-evidence-wave6.py"
NATIVE_AUTHORITY_PATH = ROOT / "scripts/postgres-native-restore-hosted-report.py"

PLATFORMS = {
    "native-linux": "native-linux-x86_64",
    "native-windows": "native-windows-x64",
    "oci-linux": "oci-linux-amd64",
    "macos-source": "macos-source",
    "security-observability": "all-supported-platforms",
}

TELEMETRY_HTTPS_CHECKS = {
    "trusted_ca": True,
    "hostname_verification": True,
    "otlp_https_delivery": True,
    "untrusted_ca_rejected": True,
    "hostname_mismatch_rejected": True,
    "non_success_rejected": True,
    "oversized_response_rejected": True,
    "slow_response_bounded": True,
    "endpoint_validation": True,
}
TELEMETRY_HTTPS_TESTS = {
    "trusted_https_otlp": (
        "worldstream-server",
        "tls_transport_uses_trusted_ca_hostname_verification_and_otlp_http",
    ),
    "tls_rejection": (
        "worldstream-server",
        "tls_transport_rejects_untrusted_cert_and_hostname_mismatch",
    ),
    "local_otlp_http": (
        "worldstream-server",
        "standard_http_transport_exercises_a_local_otlp_http_collector",
    ),
    "http_failure_bounds": (
        "worldstream-server",
        "standard_http_transport_rejects_non_success_and_oversized_responses",
    ),
    "slow_response_bounds": (
        "worldstream-server",
        "standard_http_transport_slow_response_is_bounded",
    ),
    "https_configuration": (
        "worldstream-server/worldstreamd",
        "telemetry_selects_validated_https_otlp",
    ),
    "endpoint_rejection": (
        "worldstream-runtime",
        "telemetry_endpoint_rejects_credentials_queries_bad_ports_and_unsupported_schemes",
    ),
}

OCI_POSTGRES_IMAGE = (
    "postgres:17.11-alpine@"
    "sha256:18cfe3ef5e6815560c98237d6216d1e5119702fb0f3894c8785dd58b8bbe5d73"
)
OCI_POSTGRES_IDENTITY = "postgresql/17.11; server_version_num=170011"
MACOS_ARCHITECTURES = ("arm64", "x86_64")
MACOS_BROWSER_IDENTITIES = {
    "arm64": {
        "product": "chrome-for-testing-headless-shell",
        "version": "152.0.7977.54",
        "sha256": "sha256:4e0c165ef2f0d7265fb1e6b3df2d03d1d6581fb72cdfcebeac19c09760571df6",
        "size_bytes": 167_333_040,
        "version_output": "Google Chrome for Testing 152.0.7977.54",
        "distribution": {
            "url": "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-arm64/chrome-headless-shell-mac-arm64.zip",
            "sha256": "sha256:ef5d61434f13d9d2d9bdc7c9ab4bff92225979e196458cf846640862b25f127d",
            "size_bytes": 98_034_515,
        },
    },
    "x86_64": {
        "product": "chrome-for-testing-headless-shell",
        "version": "152.0.7977.54",
        "sha256": "sha256:49b6e6bdc4a9a14a160a2fcb08576ec3acef1d360cfa61bdc79a400a27a16d99",
        "size_bytes": 182_459_908,
        "version_output": "Google Chrome for Testing 152.0.7977.54",
        "distribution": {
            "url": "https://storage.googleapis.com/chrome-for-testing-public/152.0.7977.54/mac-x64/chrome-headless-shell-mac-x64.zip",
            "sha256": "sha256:a50c716727adf9e4af5b8861d19da23b672b4687a0d631ec4beebb3009d6db97",
            "size_bytes": 102_893_170,
        },
    },
}
OCI_SECRET_SCAN_CHANNELS = {
    "image-config",
    "image-history",
    "negative-probes",
    "sqlite-container-config",
    "sqlite-container-logs",
    "postgres-container-config",
    "postgres-container-logs",
    "provider-container-config",
    "provider-container-logs",
    "probe-output",
    "runtime-report",
}
NATIVE_RESTORE_DURABLE_DOMAINS = (
    "schema_migrations",
    "operation_guards",
    "room_roots",
    "genesis",
    "materializations",
    "member_delivery_state",
    "timers",
    "transitions",
    "frames",
    "observation_consequences",
    "activation_decisions",
    "activation_intents",
    "activation_operation_receipts",
    "semantic_receipts",
    "integrity_incidents",
    "authority_fences",
    "retired_authority_fences",
    "authority_state",
    "authority_principals",
    "authority_runners",
    "authority_capabilities",
    "authority_capability_scopes",
    "authority_runner_capability_memberships",
    "authority_change_receipts",
    "authority_audit",
    "transfer_imports",
    "transfer_chunks",
    "transfer_target_fence",
    "deployment_metadata",
    "deployment_identity_metadata",
    "deployment_pack_identities",
    "deployment_resource_identities",
    "deployment_resource_blobs",
)
NATIVE_RESTORE_REPORT_FIELDS = {
    "schema",
    "status",
    "reason",
    "release_evidence",
    "native_dump_restore",
    "backup_id",
    "native_point_digest",
    "native_dump_digest",
    "native_dump_size_bytes",
    "source_provider_identity",
    "target_provider_identity",
    "verifier",
    "source_unchanged",
    "exact_restored_row_set",
    "snapshots_disposable",
    "target_isolated",
    "target_published",
    "cleanup_required",
    "native_witness_minted",
    "secrets_emitted",
    "source_version_num",
    "restored_version_num",
    "semantic_receipts_verified",
    "activation_intents_verified",
    "activation_operation_receipts_verified",
    "activation_request_evidence",
    "authority_state_verified",
    "durable_domains_verified",
    "verifier_scope",
    "source_durable_domains_digest",
    "restored_durable_domains_digest",
    "durable_domain_inventory",
    "restored_snapshot_count_before",
    "restored_snapshot_count_after",
}
HOSTED_NATIVE_RESTORE_SCHEMA = "worldstream/hosted-native-postgres-restore-evidence/v3"
PRIVATE_NATIVE_BINDING_SCHEMA = "worldstream/hosted-native-postgres-private-binding/v1"
PRIVATE_NATIVE_BINDING_ROOT = "worldstream-native-platform-binding"
PRIVATE_NATIVE_BINDING_FILES = {
    "artifact_directory_stdout": "artifact-directory.stdout",
    "snapshot_rebuild_stdout": "snapshot-rebuild.stdout",
    "native_restore_stdout": "native-restore.stdout",
    "native_report": "native-restore-report.json",
    "native_dump": "native-restore.dump",
}
MAX_RELEASE_JSON_BYTES = 64 * 1024 * 1024
MAX_RELEASE_ARTIFACT_BYTES = 8 * 1024 * 1024 * 1024
MAX_PRIVATE_DUMP_BYTES = 8 * 1024 * 1024 * 1024
ASCII_DECIMAL = re.compile(r"^[0-9]+$")
MAX_POSTGRES_SYSTEM_IDENTIFIER = (1 << 64) - 1
MAX_POSTGRES_DATABASE_OID = (1 << 32) - 1
UNTRUSTED_FIXTURE_CLASSIFICATION = "untrusted_source_bound_input_construction"
BLAKE3_DIGEST = re.compile(r"^[0-9a-f]{64}$")
SHA256_REF = re.compile(r"^sha256:[0-9a-f]{64}$")
GIT_REVISION = re.compile(r"^[0-9a-f]{40}$")


class DiagnosticError(RuntimeError):
    """An input cannot support a platform diagnostic."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise DiagnosticError(message)


def load_build_identity():
    name = "worldstream_release_platform_strict_json"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, BUILD_IDENTITY_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise DiagnosticError(f"cannot load verifier: {BUILD_IDENTITY_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


BUILD_IDENTITY = load_build_identity()


def load_blake3_module():
    name = "worldstream_release_platform_blake3"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, BLAKE3_IMPLEMENTATION_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise DiagnosticError(
            f"cannot load BLAKE3 verifier: {BLAKE3_IMPLEMENTATION_PATH}"
        )
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


BLAKE3_MODULE = load_blake3_module()
BLAKE3 = BLAKE3_MODULE.blake3


def load_native_authority():
    name = "worldstream_release_platform_native_authority"
    existing = sys.modules.get(name)
    if existing is not None:
        return existing
    spec = importlib.util.spec_from_file_location(name, NATIVE_AUTHORITY_PATH)
    if spec is None or spec.loader is None:  # pragma: no cover
        raise DiagnosticError("cannot load native private evidence authority")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


NATIVE_AUTHORITY = load_native_authority()


class StreamingBlake3:
    """Bounded-memory BLAKE3 using the reviewed implementation's primitives."""

    def __init__(self) -> None:
        self._buffer = bytearray()
        self._chunk_count = 0
        self._cv_stack: list[list[int]] = []

    def _commit(self, chunk: bytes) -> None:
        output = BLAKE3_MODULE._chunk_output(chunk, self._chunk_count)
        value = output.chaining_value_bytes()
        total_chunks = self._chunk_count + 1
        while total_chunks & 1 == 0:
            value = BLAKE3_MODULE._parent_output(
                self._cv_stack.pop(), value
            ).chaining_value_bytes()
            total_chunks >>= 1
        self._cv_stack.append(value)
        self._chunk_count += 1

    def update(self, value: bytes) -> None:
        self._buffer.extend(value)
        while len(self._buffer) > 1024:
            chunk = bytes(self._buffer[:1024])
            del self._buffer[:1024]
            self._commit(chunk)

    def digest(self) -> bytes:
        output = BLAKE3_MODULE._chunk_output(bytes(self._buffer), self._chunk_count)
        for left in reversed(self._cv_stack):
            output = BLAKE3_MODULE._parent_output(left, output.chaining_value_bytes())
        return output.root_bytes()


def postgres_native_point_digest(dump_digest: str) -> str:
    point = {
        "PostgresNative": {
            "major": 17,
            "engine_identity": "postgresql-17.11",
            "point_id": dump_digest,
            "mechanism": "Dump",
        }
    }
    canonical = json.dumps(point, ensure_ascii=False, separators=(",", ":")).encode(
        "utf-8"
    )
    return BLAKE3(canonical).hex()


def _canonical_decimal(value: Any, maximum: int) -> bool:
    return (
        isinstance(value, str)
        and ASCII_DECIMAL.fullmatch(value) is not None
        and value == str(int(value))
        and 0 < int(value) <= maximum
    )


def provider_identity_is_canonical(identity: Any) -> bool:
    return (
        isinstance(identity, dict)
        and set(identity) == {"system_identifier", "database_oid", "database_name"}
        and _canonical_decimal(
            identity.get("system_identifier"), MAX_POSTGRES_SYSTEM_IDENTIFIER
        )
        and _canonical_decimal(identity.get("database_oid"), MAX_POSTGRES_DATABASE_OID)
        and isinstance(identity.get("database_name"), str)
        and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]{0,62}", identity["database_name"])
        is not None
    )


def provider_numeric_identity(identity: dict[str, Any]) -> tuple[int, int]:
    return (int(identity["system_identifier"]), int(identity["database_oid"]))


def regular_file(path: Path, label: str) -> Path:
    try:
        mode = path.lstat().st_mode
    except FileNotFoundError as error:
        raise DiagnosticError(f"missing {label}: {path}") from error
    except OSError as error:
        raise DiagnosticError(f"cannot inspect {label}: {error}") from error
    require(not stat.S_ISLNK(mode), f"{label} must not be a symlink")
    require(stat.S_ISREG(mode), f"{label} must be a regular file")
    return path


def _attempt_retained_closes(retained_inputs: list[Any]) -> list[BaseException]:
    """Attempt every retained close and return failures without masking a primary."""

    errors: list[BaseException] = []
    for retained in reversed(retained_inputs):
        try:
            retained.close()
        except BaseException as error:  # noqa: BLE001 - cleanup must continue
            errors.append(error)
    return errors


def _finish_retained_scope(
    *,
    primary_error: BaseException | None,
    cleanup_errors: list[BaseException],
    label: str,
) -> None:
    """Preserve validation failures, otherwise fail closed on any cleanup failure."""

    if primary_error is not None:
        if isinstance(primary_error, OSError):
            raise DiagnosticError(f"{label} failed: {primary_error}") from primary_error
        raise primary_error
    if cleanup_errors:
        error = cleanup_errors[0]
        raise DiagnosticError(f"{label} cleanup failed closed: {error}") from error


@dataclass
class RetainedJsonInput:
    """A strict JSON object, digest, and size from one retained byte authority."""

    retained: Any
    raw: bytes
    value: dict[str, Any]

    @classmethod
    def open(cls, path: Path, label: str):
        retained = None
        try:
            retained = NATIVE_AUTHORITY.RetainedFile.open(
                path,
                label,
                maximum_size=MAX_RELEASE_JSON_BYTES,
            )
            raw = NATIVE_AUTHORITY._read_pass(
                retained.descriptor, MAX_RELEASE_JSON_BYTES, label
            )
            retained.verify()
            value = BUILD_IDENTITY.strict_json(raw, label)
            return cls(retained=retained, raw=raw, value=value)
        except BUILD_IDENTITY.IdentityError as error:
            if retained is not None:
                _attempt_retained_closes([retained])
            raise DiagnosticError(str(error)) from error
        except (NATIVE_AUTHORITY.HostedReportError, OSError) as error:
            if retained is not None:
                _attempt_retained_closes([retained])
            raise DiagnosticError(f"cannot retain {label}: {error}") from error

    @property
    def sha256(self) -> str:
        return self.retained.sha256

    @property
    def size_bytes(self) -> int:
        return self.retained.size_bytes

    def verify(self) -> None:
        try:
            self.retained.verify()
            raw = NATIVE_AUTHORITY._read_pass(
                self.retained.descriptor, MAX_RELEASE_JSON_BYTES, self.retained.label
            )
        except (NATIVE_AUTHORITY.HostedReportError, OSError) as error:
            raise DiagnosticError(
                f"{self.retained.label} changed while retained: {error}"
            ) from error
        require(raw == self.raw, f"{self.retained.label} changed while retained")

    def close(self) -> None:
        self.retained.close()


@dataclass
class RetainedArtifactInput:
    """One bounded release-artifact fingerprint with retained pathname identity."""

    retained: Any

    @classmethod
    def open(cls, path: Path, label: str):
        retained = None
        try:
            retained = NATIVE_AUTHORITY.RetainedFile.open(
                path,
                label,
                maximum_size=MAX_RELEASE_ARTIFACT_BYTES,
            )
            retained.verify()
            return cls(retained=retained)
        except (NATIVE_AUTHORITY.HostedReportError, OSError) as error:
            if retained is not None:
                _attempt_retained_closes([retained])
            raise DiagnosticError(f"cannot retain {label}: {error}") from error

    @property
    def sha256(self) -> str:
        return self.retained.sha256

    @property
    def size_bytes(self) -> int:
        return self.retained.size_bytes

    def verify(self) -> None:
        try:
            self.retained.verify()
        except (NATIVE_AUTHORITY.HostedReportError, OSError) as error:
            raise DiagnosticError(
                f"{self.retained.label} changed while retained: {error}"
            ) from error

    def close(self) -> None:
        self.retained.close()


def _canonical_identity_is_valid(value: Any) -> bool:
    return NATIVE_AUTHORITY._canonical_identity_is_valid(value)


def _binding_stream_hashes(
    descriptor: int, *, maximum_size: int, label: str
) -> tuple[str, str, int]:
    os.lseek(descriptor, 0, os.SEEK_SET)
    sha256 = hashlib.sha256()
    blake3 = StreamingBlake3()
    observed = 0
    while True:
        chunk = os.read(descriptor, 1024 * 1024)
        if not chunk:
            break
        observed += len(chunk)
        require(observed <= maximum_size, f"{label} exceeds its byte bound")
        sha256.update(chunk)
        blake3.update(chunk)
    return "sha256:" + sha256.hexdigest(), "blake3:" + blake3.digest().hex(), observed


class PrivateNativeBinding:
    """Retain, verify, and finally scrub the hosted private byte authority."""

    def __init__(self) -> None:
        self.root = None
        self.records: dict[str, dict[str, Any]] = {}

    def admit(self, parent: Path, binding: Any) -> None:
        require(self.root is None, "native private binding was already admitted")
        self._validate_shape(binding)
        parent = Path(os.path.abspath(parent))
        chain: list[tuple[Path, int, os.stat_result]] = []
        root_descriptor = -1
        try:
            chain = NATIVE_AUTHORITY._retained_directory_chain(
                parent, "native private binding parent"
            )
            parent_path, parent_descriptor, parent_admitted = chain[-1]
            require(
                NATIVE_AUTHORITY._canonical_file_identity(parent_descriptor)
                == binding["parent_identity"],
                "native private binding parent identity mismatch",
            )
            if os.name != "nt":
                require(
                    parent_admitted.st_uid == os.geteuid()
                    and stat.S_IMODE(parent_admitted.st_mode) & 0o077 == 0,
                    "native private binding parent must be owner-only",
                )
            root_descriptor, root_admitted = (
                NATIVE_AUTHORITY._open_exact_child_directory(
                    parent_path,
                    parent_descriptor,
                    PRIVATE_NATIVE_BINDING_ROOT,
                    "native private binding root",
                )
            )
            self.root = NATIVE_AUTHORITY.RetainedDirectory(
                path=parent / PRIVATE_NATIVE_BINDING_ROOT,
                name=PRIVATE_NATIVE_BINDING_ROOT,
                descriptor=root_descriptor,
                admitted=root_admitted,
                parent_path=parent_path,
                parent_descriptor=parent_descriptor,
                parent_admitted=parent_admitted,
                ancestor_directories=tuple(chain[:-1]),
            )
            root_descriptor = -1
            chain = []
            self.root.verify()
            require(
                NATIVE_AUTHORITY._canonical_file_identity(self.root.descriptor)
                == binding["root_identity"],
                "native private binding root identity mismatch",
            )
            self.records = binding["files"]
            self._admit_children()
        except DiagnosticError:
            raise
        except (NATIVE_AUTHORITY.HostedReportError, OSError) as error:
            raise DiagnosticError(
                f"cannot admit native private binding: {error}"
            ) from error
        finally:
            if self.root is None:
                if root_descriptor >= 0:
                    os.close(root_descriptor)
                for _path, descriptor, _admitted in reversed(chain):
                    os.close(descriptor)

    @staticmethod
    def _validate_shape(binding: Any) -> None:
        require(
            isinstance(binding, dict)
            and set(binding)
            == {
                "schema",
                "parent_identity",
                "root_name",
                "root_identity",
                "files",
            }
            and binding.get("schema") == PRIVATE_NATIVE_BINDING_SCHEMA
            and binding.get("root_name") == PRIVATE_NATIVE_BINDING_ROOT
            and _canonical_identity_is_valid(binding.get("parent_identity"))
            and _canonical_identity_is_valid(binding.get("root_identity"))
            and isinstance(binding.get("files"), dict)
            and set(binding["files"]) == set(PRIVATE_NATIVE_BINDING_FILES),
            "native private binding manifest is malformed",
        )
        identities: list[tuple[str, str]] = []
        for key, expected_name in PRIVATE_NATIVE_BINDING_FILES.items():
            record = binding["files"].get(key)
            maximum_size = (
                MAX_PRIVATE_DUMP_BYTES
                if key == "native_dump"
                else MAX_RELEASE_JSON_BYTES
            )
            require(
                isinstance(record, dict)
                and set(record) == {"name", "identity", "sha256", "size_bytes"}
                and record.get("name") == expected_name
                and _canonical_identity_is_valid(record.get("identity"))
                and isinstance(record.get("sha256"), str)
                and SHA256_REF.fullmatch(record["sha256"]) is not None
                and type(record.get("size_bytes")) is int
                and 0 < record["size_bytes"] <= maximum_size,
                f"native private binding record is malformed: {key}",
            )
            identities.append(
                (record["identity"]["storage_id"], record["identity"]["file_id"])
            )
        require(
            len(set(identities)) == len(identities),
            "native private binding artifact identities must be distinct",
        )

    def _admit_children(self) -> None:
        assert self.root is not None
        names = self.root.list_names()
        require(
            len(names) <= 16
            and set(PRIVATE_NATIVE_BINDING_FILES.values()).issubset(names),
            "native private binding file set is incomplete or unbounded",
        )
        errors: list[BaseException] = []
        for key, name in PRIVATE_NATIVE_BINDING_FILES.items():
            try:
                self.root.retain_child(
                    name,
                    expected_identity=self.records[key]["identity"],
                    writable=True,
                )
            except (NATIVE_AUTHORITY.HostedReportError, OSError) as error:
                errors.append(error)
        if errors:
            raise DiagnosticError(
                f"native private binding child identity mismatch: {errors[0]}"
            )
        require(
            set(names) == set(PRIVATE_NATIVE_BINDING_FILES.values()),
            "native private binding contains unexpected files",
        )
        for key, name in PRIVATE_NATIVE_BINDING_FILES.items():
            descriptor, admitted, _identity = self.root.retained_children[name]
            record = self.records[key]
            maximum_size = (
                MAX_PRIVATE_DUMP_BYTES
                if key == "native_dump"
                else MAX_RELEASE_JSON_BYTES
            )
            first = _binding_stream_hashes(
                descriptor, maximum_size=maximum_size, label=f"private {key}"
            )
            second = _binding_stream_hashes(
                descriptor, maximum_size=maximum_size, label=f"private {key}"
            )
            require(
                first == second
                and first[0] == record["sha256"]
                and first[2] == record["size_bytes"] == admitted.st_size,
                f"native private binding content mismatch: {key}",
            )

    def read_json(self, key: str) -> tuple[dict[str, Any], bytes]:
        assert self.root is not None
        name = PRIVATE_NATIVE_BINDING_FILES[key]
        raw = self.root.read_retained_child(
            name,
            maximum_size=MAX_RELEASE_JSON_BYTES,
            label=f"private {key}",
        )
        record = self.records[key]
        require(
            len(raw) == record["size_bytes"]
            and "sha256:" + hashlib.sha256(raw).hexdigest() == record["sha256"],
            f"native private binding content mismatch: {key}",
        )
        try:
            return BUILD_IDENTITY.strict_json(raw, f"private {key}"), raw
        except BUILD_IDENTITY.IdentityError as error:
            raise DiagnosticError(str(error)) from error

    def dump_hashes(self) -> tuple[str, str, int]:
        assert self.root is not None
        name = PRIVATE_NATIVE_BINDING_FILES["native_dump"]
        descriptor = self.root.retained_children[name][0]
        first = _binding_stream_hashes(
            descriptor, maximum_size=MAX_PRIVATE_DUMP_BYTES, label="private native dump"
        )
        second = _binding_stream_hashes(
            descriptor, maximum_size=MAX_PRIVATE_DUMP_BYTES, label="private native dump"
        )
        require(first == second, "private native dump changed while retained")
        return first

    def scrub_exact(self) -> None:
        if self.root is None:
            return
        retained = list(self.root.retained_children.items())
        for name, (descriptor, admitted, identity) in retained:
            opened = os.fstat(descriptor)
            require(
                stat.S_ISREG(opened.st_mode)
                and opened.st_nlink == 1
                and NATIVE_AUTHORITY._same_identity(admitted, opened)
                and NATIVE_AUTHORITY._canonical_file_identity(descriptor) == identity,
                f"native private binding changed before exact scrub: {name}",
            )
        for name, (descriptor, admitted, identity) in retained:
            os.ftruncate(descriptor, 0)
            os.fsync(descriptor)
            after = os.fstat(descriptor)
            require(
                after.st_size == 0
                and after.st_nlink == 1
                and NATIVE_AUTHORITY._same_identity(admitted, after)
                and NATIVE_AUTHORITY._canonical_file_identity(descriptor) == identity,
                f"native private binding exact scrub failed: {name}",
            )

    def close(self) -> None:
        if self.root is not None:
            self.root.close()
            self.root = None


def read_json(path: Path, label: str) -> dict[str, Any]:
    retained = None
    value = None
    primary_error: BaseException | None = None
    try:
        retained = RetainedJsonInput.open(path, label)
        retained.verify()
        value = retained.value
    except BaseException as error:  # noqa: BLE001 - close retained input on interrupts
        primary_error = error
    cleanup_errors = _attempt_retained_closes(
        [retained] if retained is not None else []
    )
    _finish_retained_scope(
        primary_error=primary_error,
        cleanup_errors=cleanup_errors,
        label=label,
    )
    assert value is not None
    return value


def digest(path: Path) -> str:
    retained = None
    value = None
    primary_error: BaseException | None = None
    try:
        retained = RetainedArtifactInput.open(path, f"digest input {path.name}")
        retained.verify()
        value = retained.sha256
    except BaseException as error:  # noqa: BLE001 - close retained input on interrupts
        primary_error = error
    cleanup_errors = _attempt_retained_closes(
        [retained] if retained is not None else []
    )
    _finish_retained_scope(
        primary_error=primary_error,
        cleanup_errors=cleanup_errors,
        label=f"cannot hash {path}",
    )
    assert value is not None
    return value


def load_script(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise DiagnosticError(f"cannot load verifier: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def independently_verify_oci_context(context: Path) -> dict[str, Any]:
    package = load_script("release_platform_package_verifier", PACKAGE_SCRIPT)
    try:
        return package.verify_oci_context(context)
    except package.PackageError as error:
        raise DiagnosticError(f"OCI context verification failed: {error}") from error


def independently_verify_oci_archive(
    artifact: Path, tested_image_config_digest: str
) -> dict[str, Any]:
    verifier = load_script("release_platform_oci_layout_verifier", OCI_LAYOUT_VERIFIER)
    try:
        return verifier.verify_artifact(artifact, tested_image_config_digest)
    except verifier.VerificationError as error:
        raise DiagnosticError(f"OCI archive verification failed: {error}") from error


def independently_verify_native_archive(
    artifact: Path, source_id: str, version: str
) -> tuple[dict[str, Any], dict[str, Any]]:
    package = load_script("release_platform_native_package_verifier", PACKAGE_SCRIPT)
    target = {
        "native-linux": "linux-x86_64",
        "native-windows": "windows-x64",
    }[source_id]
    try:
        package.verify_archive(artifact)
        report = package.archive_report(artifact)
        entries = package.archive_entries(artifact)
    except package.PackageError as error:
        raise DiagnosticError(f"native archive verification failed: {error}") from error
    require(
        report.get("identity", {}).get("target") == target
        and report.get("identity", {}).get("version") == version,
        "independently verified native archive target/version mismatch",
    )
    root = f"worldstream-{version}-{target}"
    suffix = ".exe" if source_id == "native-windows" else ""
    daemon = entries.get(f"{root}/bin/worldstreamd{suffix}")
    control = entries.get(f"{root}/bin/worldstreamctl{suffix}")
    require(
        isinstance(daemon, bytes) and daemon and isinstance(control, bytes) and control,
        "independently verified native archive has no packaged binaries",
    )
    return report, {
        "worldstreamd_sha256": "sha256:" + hashlib.sha256(daemon).hexdigest(),
        "worldstreamd_size_bytes": len(daemon),
        "worldstreamctl_sha256": "sha256:" + hashlib.sha256(control).hexdigest(),
        "worldstreamctl_size_bytes": len(control),
    }


def manifest() -> dict[str, Any]:
    try:
        with MANIFEST.open("rb") as source:
            value = tomllib.load(source)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise DiagnosticError(f"cannot read compatibility manifest: {error}") from error
    require(
        value.get("manifest_kind") == "release"
        and value.get("release_ready") is True
        and value.get("unresolved_required_fields") == [],
        "embedded release contract is incomplete",
    )
    return value


def pinned_macos_toolchains() -> dict[str, str]:
    """Read the exact repository-owned toolchain pins used by macOS evidence."""

    def text_pin(relative: str) -> str:
        path = regular_file(ROOT / relative, f"{relative} toolchain pin")
        try:
            value = path.read_text(encoding="utf-8").strip()
        except (OSError, UnicodeError) as error:
            raise DiagnosticError(f"cannot read {relative} toolchain pin") from error
        require(
            re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", value) is not None,
            f"{relative} is not an exact semantic version pin",
        )
        return value

    try:
        rust = tomllib.loads(
            regular_file(ROOT / "rust-toolchain.toml", "Rust toolchain pin").read_text(
                encoding="utf-8"
            )
        )["toolchain"]["channel"]
    except (
        OSError,
        UnicodeError,
        KeyError,
        TypeError,
        tomllib.TOMLDecodeError,
    ) as error:
        raise DiagnosticError("cannot read the exact Rust toolchain pin") from error
    require(
        isinstance(rust, str)
        and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", rust) is not None,
        "rust-toolchain.toml is not an exact semantic version pin",
    )
    package = read_json(ROOT / "package.json", "root package manifest")
    package_manager = package.get("packageManager")
    package_manager_match = (
        re.fullmatch(
            r"pnpm@(?P<version>[0-9]+\.[0-9]+\.[0-9]+)"
            r"\+sha512\.(?P<integrity>[0-9a-f]{128})",
            package_manager,
        )
        if isinstance(package_manager, str)
        else None
    )
    if package_manager_match is None:
        raise DiagnosticError(
            "root packageManager is not an exact integrity-bound pnpm pin"
        )
    return {
        "rust": rust,
        "python": text_pin(".python-version"),
        "node": text_pin(".node-version"),
        "pnpm": package_manager_match.group("version"),
        "uv": text_pin(".uv-version"),
    }


def atomic_write(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    require(not path.is_symlink() and not path.is_dir(), f"unsafe output path: {path}")
    descriptor, temporary_name = tempfile.mkstemp(
        prefix=f".{path.name}.", dir=path.parent
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            json.dump(value, output, ensure_ascii=False, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise


def diagnostic(
    source_id: str, kind: str, facts: dict[str, Any], contract: dict[str, Any]
) -> dict[str, Any]:
    return {
        "schema": SCHEMA,
        "report_id": f"{source_id}/{kind}",
        "source_id": source_id,
        "platform": PLATFORMS[source_id],
        "version": contract["release_candidate"],
        "contract": contract["contracts"],
        "status": "passed",
        "release_evidence": True,
        "fail_closed": False,
        "facts": facts,
    }


def gate_outcomes(
    report: dict[str, Any], expected_system: str, required: set[str]
) -> dict[str, dict[str, Any]]:
    require(report.get("schema") == GATE_SCHEMA, "platform gate report schema mismatch")
    require(
        report.get("tier") == "minimal-ci" and report.get("strict") is True,
        "platform gate report is not strict minimal CI",
    )
    observed_platform = report.get("platform")
    require(
        isinstance(observed_platform, str)
        and expected_system.lower() in observed_platform.lower(),
        "platform gate report native system mismatch",
    )
    outcomes = report.get("outcomes")
    require(isinstance(outcomes, list), "platform gate report outcomes are missing")
    by_name: dict[str, list[dict[str, Any]]] = {}
    for outcome in outcomes:
        require(isinstance(outcome, dict), "platform gate outcome is malformed")
        name = outcome.get("name")
        require(isinstance(name, str) and name, "platform gate outcome has no name")
        by_name.setdefault(name, []).append(outcome)
    failures = [
        outcome
        for values in by_name.values()
        for outcome in values
        if outcome.get("status") == "FAIL"
    ]
    require(not failures, "platform gate report contains a failed outcome")
    for name in required:
        values = by_name.get(name, [])
        require(
            values and all(value.get("status") == "PASS" for value in values),
            f"required platform outcome did not pass: {name}",
        )
    return {name: values[-1] for name, values in by_name.items()}


def verify_archive_report(
    report: dict[str, Any],
    artifact: Path,
    source_id: str,
    contract: dict[str, Any],
    independently_verified: dict[str, Any],
    *,
    artifact_sha256: str,
    artifact_size_bytes: int,
) -> None:
    target = {
        "native-linux": "linux-x86_64",
        "native-windows": "windows-x64",
    }[source_id]
    require(
        report.get("schema") == "worldstream/package-report/v1",
        "package report schema mismatch",
    )
    require(report.get("kind") == "archive", "package report is not an archive report")
    require(
        report.get("identity", {}).get("target") == target
        and report.get("identity", {}).get("version") == contract["release_candidate"],
        "package target/version identity mismatch",
    )
    require(
        report.get("inventory", {}).get("archive_verified") is True
        and report.get("inventory", {}).get("manifest_source") == "compatibility.toml"
        and report.get("inventory", {}).get("manifest_mirror") == "compatibility.json"
        and report.get("inventory", {}).get("release_evidence") is False,
        "package archive verification boundary is incomplete",
    )
    require(
        report.get("artifact") == artifact.name, "package report artifact name mismatch"
    )
    require(
        report.get("sha256") == artifact_sha256,
        "package report archive digest mismatch",
    )
    require(
        report.get("path") == artifact.name
        and report.get("size_bytes") == artifact_size_bytes,
        "package report archive size mismatch",
    )
    require(
        report == independently_verified,
        "package report is not the exact independently verified archive projection",
    )


def verify_native_runtime_report(
    report: dict[str, Any],
    package_report: dict[str, Any],
    package_report_sha256: str,
    package_report_size_bytes: int,
    artifact: Path,
    source_id: str,
    archive_binaries: dict[str, Any],
    *,
    artifact_sha256: str,
    artifact_size_bytes: int,
) -> dict[str, Any]:
    expected_target = {
        "native-linux": "linux-x86_64",
        "native-windows": "windows-x64",
    }[source_id]
    expected_system = "Linux" if source_id == "native-linux" else "Windows"
    expected_machines = {"x86_64", "amd64"}
    binding = report.get("package_binding")
    profiles = report.get("profiles")
    postgres_admin = report.get("postgres_admin")
    transfer_operator = report.get("transfer_operator")
    control_output = report.get("control_output")
    package_identity = package_report.get("identity")
    require(
        set(report)
        == {
            "schema",
            "status",
            "release_evidence",
            "secrets_emitted",
            "platform",
            "package_binding",
            "profiles",
            "postgres_admin",
            "transfer_operator",
            "control_output",
            "cleanup",
        }
        and report.get("schema") == "worldstream/native-package-runtime-smoke/v1"
        and report.get("status") == "pass"
        and report.get("release_evidence") is False
        and report.get("secrets_emitted") is False
        and report.get("cleanup") == "pass"
        and report.get("platform", {}).get("system") == expected_system
        and str(report.get("platform", {}).get("machine", "")).lower()
        in expected_machines,
        "native packaged runtime smoke is incomplete",
    )
    require(
        isinstance(binding, dict)
        and set(binding)
        == {
            "artifact",
            "target",
            "version",
            "archive_sha256",
            "archive_size_bytes",
            "package_report_sha256",
            "package_report_size_bytes",
            "manifest_sha256",
            "manifest_json_sha256",
            "manifest_toml_sha256",
            "source_revision",
            "build_identity_sha256",
            "worldstreamd_sha256",
            "worldstreamd_size_bytes",
            "worldstreamctl_sha256",
            "worldstreamctl_size_bytes",
            "canonical_archive_verified",
            "exact_archive_bytes_executed",
        }
        and binding.get("artifact") == artifact.name
        and binding.get("target") == expected_target
        and binding.get("version") == package_identity.get("version")
        and binding.get("archive_sha256") == artifact_sha256
        and binding.get("archive_size_bytes") == artifact_size_bytes
        and binding.get("package_report_sha256") == package_report_sha256
        and binding.get("package_report_size_bytes") == package_report_size_bytes
        and binding.get("manifest_sha256")
        == "sha256:" + package_identity.get("manifest_sha256", "")
        and binding.get("manifest_json_sha256")
        == "sha256:" + package_identity.get("manifest_json_sha256", "")
        and binding.get("manifest_toml_sha256")
        == "sha256:" + package_identity.get("manifest_toml_sha256", "")
        and isinstance(binding.get("source_revision"), str)
        and GIT_REVISION.fullmatch(binding["source_revision"]) is not None
        and binding.get("source_revision") == package_identity.get("source_revision")
        and isinstance(binding.get("build_identity_sha256"), str)
        and SHA256_REF.fullmatch(binding["build_identity_sha256"]) is not None
        and binding.get("build_identity_sha256")
        == package_identity.get("build_identity_sha256")
        and isinstance(binding.get("worldstreamd_sha256"), str)
        and SHA256_REF.fullmatch(binding["worldstreamd_sha256"])
        and type(binding.get("worldstreamd_size_bytes")) is int
        and binding["worldstreamd_size_bytes"] > 0
        and isinstance(binding.get("worldstreamctl_sha256"), str)
        and SHA256_REF.fullmatch(binding["worldstreamctl_sha256"])
        and type(binding.get("worldstreamctl_size_bytes")) is int
        and binding["worldstreamctl_size_bytes"] > 0
        and all(
            binding.get(field) == value for field, value in archive_binaries.items()
        )
        and binding.get("canonical_archive_verified") is True
        and binding.get("exact_archive_bytes_executed") is True,
        "native packaged runtime smoke is not bound to the exact archive/report/manifests/binaries",
    )
    require(
        isinstance(profiles, dict)
        and set(profiles) == {"sqlite-bundled", "postgres-primary"},
        "native packaged runtime smoke did not run both storage profiles",
    )
    for profile, value in profiles.items():
        expected_engine_prefix = (
            "sqlite/" if profile == "sqlite-bundled" else "postgresql/17.11;"
        )
        require(
            isinstance(value, dict)
            and set(value)
            == (
                {
                    "status",
                    "healthz",
                    "readyz",
                    "version",
                    "engine_identity",
                    "packaged_ctl",
                }
                | ({"sqlite_operator"} if profile == "sqlite-bundled" else set())
            )
            and value.get("status") == "pass"
            and value.get("healthz") == "pass"
            and value.get("readyz") == "pass"
            and value.get("version") == "manifest_and_engine_exact"
            and isinstance(value.get("engine_identity"), str)
            and value["engine_identity"].startswith(expected_engine_prefix)
            and value.get("packaged_ctl")
            == {
                "config_validate": "pass",
                "config_effective": "redacted_and_precedence_exact",
                "doctor": "bounded_diagnostics_exposed",
                "health": "pass",
                "version": "pass",
                "binary_version": package_identity["version"],
                "source_revision": binding["source_revision"],
            },
            f"native packaged runtime smoke {profile} contract is incomplete",
        )
        if profile == "sqlite-bundled":
            require(
                value.get("sqlite_operator")
                == {
                    "backup": "native_and_semantic_pass",
                    "restore": "native_and_semantic_pass",
                    "verify": "native_only_pass_semantic_not_invoked",
                    "envelope": "exact_bytes_preserved",
                },
                "native packaged SQLite operator backup/restore contract is incomplete",
            )
    require(
        postgres_admin
        == {
            "server_version_num": "170011",
            "dsn_delivery": "owner_only_file",
            "migrate": "pass",
            "verify": "pass",
            "runtime_role": "least_privilege",
        },
        "native packaged PostgreSQL administration contract is incomplete",
    )
    control_binding = {
        name: binding[name]
        for name in (
            "archive_sha256",
            "archive_size_bytes",
            "package_report_sha256",
            "package_report_size_bytes",
            "worldstreamctl_sha256",
            "worldstreamctl_size_bytes",
        )
    }
    require(
        isinstance(transfer_operator, dict)
        and set(transfer_operator)
        == {"status", "control_binding", "abort", "finalized"}
        and transfer_operator.get("status") == "pass"
        and transfer_operator.get("control_binding") == control_binding,
        "native packaged transfer operator is not bound to the exact archive and control binary",
    )
    abort = transfer_operator.get("abort")
    require(
        isinstance(abort, dict)
        and set(abort)
        == {
            "restart_checkpoint",
            "pre_abort_checkpoint_rows",
            "pre_abort_checkpoint_bytes",
            "provider_cleanup",
            "source_authority_restored",
            "abort_replay",
            "bundle_sha256",
            "bundle_size_bytes",
        }
        and abort.get("restart_checkpoint") == "pass"
        and abort.get("pre_abort_checkpoint_rows") == 1
        and type(abort.get("pre_abort_checkpoint_bytes")) is int
        and abort["pre_abort_checkpoint_bytes"] > 0
        and abort.get("provider_cleanup")
        == "all_durable_domains_empty_with_exact_tombstone"
        and abort.get("source_authority_restored") == "pass"
        and abort.get("abort_replay") == "pass"
        and isinstance(abort.get("bundle_sha256"), str)
        and SHA256_REF.fullmatch(abort["bundle_sha256"]) is not None
        and type(abort.get("bundle_size_bytes")) is int
        and abort["bundle_size_bytes"] > 0,
        "native packaged transfer abort/restart contract is incomplete",
    )
    finalized = transfer_operator.get("finalized")
    require(
        isinstance(finalized, dict)
        and set(finalized)
        == {
            "restart_resume",
            "resume_invocations",
            "completed_resume_replay",
            "canonical_bundle_sha256",
            "canonical_bundle_size_bytes",
            "operator_bundle_hash",
            "record_count",
            "target_epoch",
            "hydration_verified_before_handoff",
            "source_retired",
            "target_authoritative",
            "finalize_replay",
        }
        and finalized.get("restart_resume") == "pass"
        and type(finalized.get("resume_invocations")) is int
        and finalized["resume_invocations"] > 1
        and finalized.get("completed_resume_replay") == "no_new_generation"
        and isinstance(finalized.get("canonical_bundle_sha256"), str)
        and SHA256_REF.fullmatch(finalized["canonical_bundle_sha256"]) is not None
        and type(finalized.get("canonical_bundle_size_bytes")) is int
        and finalized["canonical_bundle_size_bytes"] > 0
        and isinstance(finalized.get("operator_bundle_hash"), str)
        and BLAKE3_DIGEST.fullmatch(finalized["operator_bundle_hash"]) is not None
        and type(finalized.get("record_count")) is int
        and finalized["record_count"] > 1
        and type(finalized.get("target_epoch")) is int
        and finalized["target_epoch"] > 1
        and finalized.get("hydration_verified_before_handoff") == "pass"
        and finalized.get("source_retired") == "pass"
        and finalized.get("target_authoritative") == "pass"
        and finalized.get("finalize_replay") == "pass",
        "native packaged transfer hydration/finalization contract is incomplete",
    )
    require(
        control_output
        == {
            "status": "persisted",
            **control_binding,
        },
        "native packaged retained control output is incomplete or unbound",
    )
    return binding


def verify_native_restore_report(report: dict[str, Any]) -> dict[str, Any]:
    provider_identities = (
        report.get("source_provider_identity"),
        report.get("target_provider_identity"),
    )

    require(
        set(report) == NATIVE_RESTORE_REPORT_FIELDS,
        "native PostgreSQL restore report has wrong fields",
    )
    require(
        report.get("schema") == "worldstream/native-postgres-restore-evidence/v2"
        and report.get("status") == "ready"
        and report.get("reason")
        == "postgres_native_restore_verified_by_unified_verifier"
        and report.get("release_evidence") is False
        and report.get("native_dump_restore") == "pass"
        and isinstance(report.get("native_dump_digest"), str)
        and BLAKE3_DIGEST.fullmatch(report["native_dump_digest"]) is not None
        and report.get("backup_id") == "postgres-native-" + report["native_dump_digest"]
        and isinstance(report.get("native_point_digest"), str)
        and BLAKE3_DIGEST.fullmatch(report["native_point_digest"]) is not None
        and report["native_point_digest"]
        == postgres_native_point_digest(report["native_dump_digest"])
        and type(report.get("native_dump_size_bytes")) is int
        and report["native_dump_size_bytes"] > 0
        and all(provider_identity_is_canonical(value) for value in provider_identities)
        and provider_numeric_identity(provider_identities[0])
        != provider_numeric_identity(provider_identities[1])
        and report.get("source_unchanged") is True
        and report.get("exact_restored_row_set") is True
        and report.get("snapshots_disposable") is True
        and report.get("target_isolated") is True
        and report.get("target_published") is False
        and report.get("cleanup_required") is True
        and report.get("native_witness_minted") is True
        and report.get("secrets_emitted") is False
        and report.get("source_version_num") == 170_011
        and report.get("restored_version_num") == 170_011
        and report.get("semantic_receipts_verified") is True
        and report.get("activation_intents_verified") is True
        and report.get("activation_operation_receipts_verified") is True
        and report.get("activation_request_evidence")
        == "stored_canonical_hash_only_verified"
        and report.get("authority_state_verified") is True
        and report.get("durable_domains_verified") is True
        and type(report.get("restored_snapshot_count_before")) is int
        and report["restored_snapshot_count_before"] > 0
        and type(report.get("restored_snapshot_count_after")) is int
        and report["restored_snapshot_count_after"] == 0,
        "native PostgreSQL restore did not pass the isolated semantic contract",
    )
    scope = report.get("verifier_scope")
    require(
        isinstance(scope, dict)
        and all(
            type(scope.get(field)) is int
            for field in (
                "source_pack_identity_count",
                "restored_pack_identity_count",
                "source_resource_identity_count",
                "restored_resource_identity_count",
                "source_fired_timer_count",
                "restored_fired_timer_count",
            )
        )
        and scope
        == {
            "profile": "full_deployment_all_durable_domains",
            "source_pack_identity_count": 2,
            "restored_pack_identity_count": 2,
            "source_resource_identity_count": 1,
            "restored_resource_identity_count": 1,
            "source_fired_timer_count": 1,
            "restored_fired_timer_count": 1,
            "general_deployment_support_verified": True,
        },
        "native PostgreSQL restore verifier scope is incomplete or stale",
    )
    source_digest = report.get("source_durable_domains_digest")
    require(
        isinstance(source_digest, str)
        and BLAKE3_DIGEST.fullmatch(source_digest) is not None
        and source_digest == report.get("restored_durable_domains_digest"),
        "native PostgreSQL restore durable-domain aggregate mismatch",
    )
    inventory = report.get("durable_domain_inventory")
    require(
        isinstance(inventory, list)
        and len(inventory) == len(NATIVE_RESTORE_DURABLE_DOMAINS),
        "native PostgreSQL restore durable-domain inventory is incomplete",
    )
    observed_domains = []
    for row in inventory:
        require(
            isinstance(row, dict)
            and set(row)
            == {
                "domain",
                "source_row_count",
                "restored_row_count",
                "source_digest",
                "restored_digest",
            }
            and isinstance(row.get("domain"), str)
            and type(row.get("source_row_count")) is int
            and row["source_row_count"] >= 0
            and type(row.get("restored_row_count")) is int
            and row["restored_row_count"] == row["source_row_count"]
            and isinstance(row.get("source_digest"), str)
            and BLAKE3_DIGEST.fullmatch(row["source_digest"]) is not None
            and row.get("restored_digest") == row["source_digest"],
            "native PostgreSQL restore durable-domain row is malformed or mismatched",
        )
        observed_domains.append(row["domain"])
    require(
        observed_domains == list(NATIVE_RESTORE_DURABLE_DOMAINS),
        "native PostgreSQL restore durable-domain inventory drifted",
    )
    authority = inventory[NATIVE_RESTORE_DURABLE_DOMAINS.index("authority_state")]
    require(
        authority.get("source_row_count") == 1,
        "native PostgreSQL restore authority singleton is missing",
    )
    verifier = report.get("verifier")
    require(
        isinstance(verifier, dict)
        and set(verifier) == {"readiness", "rooms", "diagnostics"}
        and verifier.get("readiness") == "Ready",
        "native PostgreSQL provider-neutral verifier is not ready",
    )
    rooms = verifier.get("rooms")
    require(
        isinstance(rooms, dict)
        and 0 < len(rooms) <= 100_000
        and all(
            isinstance(room_id, str)
            and 0 < len(room_id) <= 512
            and disposition in {"Verified", "IsolatedPreExisting"}
            for room_id, disposition in rooms.items()
        ),
        "native PostgreSQL verifier rooms are blocked, malformed, or unbounded",
    )
    diagnostics = verifier.get("diagnostics")
    require(
        isinstance(diagnostics, list) and len(diagnostics) <= 256,
        "native PostgreSQL verifier diagnostics are malformed or unbounded",
    )
    for diagnostic_row in diagnostics:
        require(
            isinstance(diagnostic_row, dict)
            and set(diagnostic_row)
            == {"class", "code", "subject", "action", "disposition"}
            and diagnostic_row.get("class") == "Integrity"
            and diagnostic_row.get("code") == "pre_existing_isolation_preserved"
            and isinstance(diagnostic_row.get("subject"), str)
            and re.fullmatch(r"subject:[0-9a-f]{12}", diagnostic_row["subject"])
            is not None
            and diagnostic_row.get("action")
            == "keep the Room isolated; investigate or repair it through the separate verifier-repair contract"
            and diagnostic_row.get("disposition") == "PermittedPreExistingIsolation",
            "native PostgreSQL verifier contains a malformed or blocking diagnostic",
        )
    return {
        "provider_identity": "postgresql/17.11; server_version_num=170011",
        "restore_profile": scope["profile"],
        "verified_scope": "two Pack identities, one immutable resource, one fired Timer, and every durable domain",
        "target_safety": "isolated, unpublished, durably sealed, and caller-cleanup-required",
        "source_durable_domains_digest": source_digest,
        "backup_id": report["backup_id"],
        "native_point_digest": report["native_point_digest"],
        "native_dump_digest": report["native_dump_digest"],
        "native_dump_size_bytes": report["native_dump_size_bytes"],
        "source_provider_identity": report["source_provider_identity"],
        "target_provider_identity": report["target_provider_identity"],
    }


def verify_hosted_native_restore_report(
    report: dict[str, Any],
    *,
    source_id: str,
    package_report_sha256: str,
    package_report_size_bytes: int,
    artifact_sha256: str,
    artifact_size_bytes: int,
    runtime_binding: dict[str, Any],
    fixture_report_sha256: str,
    fixture_report_size_bytes: int,
    fixture_report: dict[str, Any],
    private_observations: dict[str, Any],
) -> dict[str, Any]:
    expected_system = "Linux" if source_id == "native-linux" else "Windows"
    expected_machines = {"x86_64", "amd64"}
    require(
        set(report)
        == {
            "schema",
            "status",
            "release_evidence",
            "secrets_emitted",
            "platform",
            "package_binding",
            "source_fixture",
            "product_execution",
            "native_restore",
            "cleanup",
            "private_binding",
        }
        and report.get("schema") == HOSTED_NATIVE_RESTORE_SCHEMA
        and report.get("status") == "pass"
        and report.get("release_evidence") is False
        and report.get("secrets_emitted") is False,
        "hosted native PostgreSQL restore wrapper is incomplete",
    )
    require(
        isinstance(private_observations, dict)
        and set(private_observations)
        == {"manifest", "actions", "native_report", "native_dump"}
        and report.get("private_binding") == private_observations["manifest"],
        "hosted native PostgreSQL private binding was not retained exactly",
    )
    platform_value = report.get("platform")
    require(
        isinstance(platform_value, dict)
        and set(platform_value) == {"system", "machine"}
        and platform_value.get("system") == expected_system
        and str(platform_value.get("machine", "")).lower() in expected_machines,
        "hosted native PostgreSQL restore platform identity mismatch",
    )
    expected_package_binding = {
        "archive_sha256": runtime_binding["archive_sha256"],
        "archive_size_bytes": runtime_binding["archive_size_bytes"],
        "package_report_sha256": runtime_binding["package_report_sha256"],
        "package_report_size_bytes": runtime_binding["package_report_size_bytes"],
        "source_revision": runtime_binding["source_revision"],
        "worldstreamctl_sha256": runtime_binding["worldstreamctl_sha256"],
        "worldstreamctl_size_bytes": runtime_binding["worldstreamctl_size_bytes"],
    }
    require(
        report.get("package_binding") == expected_package_binding
        and expected_package_binding["archive_sha256"] == artifact_sha256
        and expected_package_binding["archive_size_bytes"] == artifact_size_bytes
        and expected_package_binding["package_report_sha256"] == package_report_sha256
        and expected_package_binding["package_report_size_bytes"]
        == package_report_size_bytes,
        "hosted native PostgreSQL restore is not bound to the exact package/control bytes",
    )
    construction = fixture_report.get("source_construction")
    require(
        fixture_report.get("schema")
        == "worldstream/sqlite-postgresql-transfer-evidence/v1"
        and construction
        == {
            "classification": UNTRUSTED_FIXTURE_CLASSIFICATION,
            "source_revision": runtime_binding["source_revision"],
            "trusted_product_execution": False,
        }
        and fixture_report.get("status") == "pass"
        and fixture_report.get("release_evidence") is False
        and fixture_report.get("secrets_emitted") is False,
        "native PostgreSQL source fixture is not explicitly untrusted and source-bound",
    )
    expected_fixture = {
        "classification": UNTRUSTED_FIXTURE_CLASSIFICATION,
        "fixture_report_sha256": fixture_report_sha256,
        "fixture_report_size_bytes": fixture_report_size_bytes,
        "source_revision": runtime_binding["source_revision"],
    }
    require(
        report.get("source_fixture") == expected_fixture,
        "hosted native PostgreSQL restore fixture binding mismatch",
    )
    product_execution = report.get("product_execution")
    require(
        isinstance(product_execution, dict)
        and set(product_execution)
        == {
            "controller",
            "provider_tools",
            "source",
            "target",
            "actions",
            "native_report_sha256",
            "native_report_size_bytes",
            "native_report_canonical_sha256",
        },
        "hosted native PostgreSQL product execution binding is incomplete",
    )
    require(
        product_execution.get("controller")
        == {
            "execution": "retained_exact_packaged_binary",
            "sha256": runtime_binding["worldstreamctl_sha256"],
            "size_bytes": runtime_binding["worldstreamctl_size_bytes"],
        },
        "hosted native PostgreSQL execution did not retain the packaged control",
    )
    provider_tools = product_execution.get("provider_tools")
    require(
        isinstance(provider_tools, dict)
        and set(provider_tools) == {"pg_dump", "pg_restore", "psql"}
        and all(
            isinstance(provider_tools.get(tool), dict)
            and set(provider_tools[tool]) == {"sha256", "size_bytes"}
            and isinstance(provider_tools[tool].get("sha256"), str)
            and SHA256_REF.fullmatch(provider_tools[tool]["sha256"]) is not None
            and type(provider_tools[tool].get("size_bytes")) is int
            and provider_tools[tool]["size_bytes"] > 0
            for tool in ("pg_dump", "pg_restore", "psql")
        ),
        "hosted native PostgreSQL provider-tool binding is incomplete",
    )
    source = product_execution.get("source")
    target = product_execution.get("target")
    for endpoint, label in ((source, "source"), (target, "target")):
        require(
            isinstance(endpoint, dict)
            and set(endpoint) == {"host", "port", "database", "username", "tls_mode"}
            and endpoint.get("host") == "127.0.0.1"
            and type(endpoint.get("port")) is int
            and 0 < endpoint["port"] <= 65_535
            and isinstance(endpoint.get("database"), str)
            and endpoint["database"]
            and isinstance(endpoint.get("username"), str)
            and endpoint["username"]
            and endpoint.get("tls_mode") in {"require", "disable"},
            f"hosted native PostgreSQL {label} execution coordinates are invalid",
        )
    require(
        (source["host"], source["port"], source["database"])
        != (target["host"], target["port"], target["database"]),
        "hosted native PostgreSQL execution used one database as both endpoints",
    )
    actions = product_execution.get("actions")
    require(
        isinstance(actions, list) and len(actions) == 3,
        "hosted native PostgreSQL product action sequence is incomplete",
    )
    action_fields = {
        "operation",
        "status",
        "exit_code",
        "stdout_sha256",
        "stdout_size_bytes",
        "stderr_sha256",
        "stderr_size_bytes",
        "environment",
        "receipt",
    }
    raw_actions = private_observations.get("actions")
    require(
        isinstance(raw_actions, list) and len(raw_actions) == 3,
        "hosted native PostgreSQL raw action binding is incomplete",
    )
    for index, (action, operation) in enumerate(
        zip(
            actions,
            ("artifact_directory_identity", "snapshot_rebuild", "native_restore"),
            strict=True,
        )
    ):
        raw_action = raw_actions[index]
        require(
            isinstance(action, dict)
            and set(action) == action_fields
            and action.get("operation") == operation
            and action.get("status") == "pass"
            and type(action.get("exit_code")) is int
            and action["exit_code"] == 0
            and action.get("environment") == "sanitized_no_ambient_pg"
            and isinstance(action.get("stdout_sha256"), str)
            and SHA256_REF.fullmatch(action["stdout_sha256"]) is not None
            and type(action.get("stdout_size_bytes")) is int
            and 0 < action["stdout_size_bytes"] <= 8 * 1024 * 1024
            and action.get("stderr_sha256")
            == "sha256:" + hashlib.sha256(b"").hexdigest()
            and type(action.get("stderr_size_bytes")) is int
            and action["stderr_size_bytes"] == 0
            and isinstance(raw_action, dict)
            and set(raw_action) == {"receipt", "sha256", "size_bytes"}
            and raw_action.get("receipt") == action.get("receipt")
            and raw_action.get("sha256") == action.get("stdout_sha256")
            and raw_action.get("size_bytes") == action.get("stdout_size_bytes"),
            "hosted native PostgreSQL product action did not pass exactly",
        )
    directory_receipt = actions[0].get("receipt")
    require(
        isinstance(directory_receipt, dict)
        and set(directory_receipt)
        == {
            "schema",
            "status",
            "artifact_directory_identity",
            "secrets_emitted",
        }
        and directory_receipt.get("schema")
        == "worldstream/postgres-native-artifact-directory-identity/v1"
        and directory_receipt.get("status") == "observed"
        and isinstance(directory_receipt.get("artifact_directory_identity"), dict)
        and set(directory_receipt["artifact_directory_identity"])
        == {"storage_id", "file_id"}
        and isinstance(
            directory_receipt["artifact_directory_identity"].get("storage_id"),
            str,
        )
        and re.fullmatch(
            r"[0-9a-f]{16}",
            directory_receipt["artifact_directory_identity"]["storage_id"],
        )
        is not None
        and isinstance(
            directory_receipt["artifact_directory_identity"].get("file_id"),
            str,
        )
        and re.fullmatch(
            r"[0-9a-f]{32}",
            directory_receipt["artifact_directory_identity"]["file_id"],
        )
        is not None
        and directory_receipt.get("secrets_emitted") is False,
        "hosted native PostgreSQL artifact directory identity receipt is incomplete",
    )
    snapshot_receipt = actions[1].get("receipt")
    require(
        isinstance(snapshot_receipt, dict)
        and set(snapshot_receipt)
        == {
            "schema",
            "status",
            "rebuilt_snapshot_count",
            "source_provider_identity",
            "secrets_emitted",
        }
        and snapshot_receipt.get("schema")
        == "worldstream/postgres-native-snapshot-rebuild-receipt/v1"
        and snapshot_receipt.get("status") == "complete"
        and type(snapshot_receipt.get("rebuilt_snapshot_count")) is int
        and snapshot_receipt["rebuilt_snapshot_count"] > 0
        and isinstance(snapshot_receipt.get("source_provider_identity"), dict)
        and set(snapshot_receipt["source_provider_identity"])
        == {"system_identifier", "database_oid", "database_name"}
        and snapshot_receipt.get("secrets_emitted") is False,
        "hosted native PostgreSQL snapshot rebuild receipt is incomplete",
    )
    native_restore = report.get("native_restore")
    raw_native = private_observations.get("native_report")
    require(
        isinstance(native_restore, dict),
        "hosted native PostgreSQL restore has no raw verifier report",
    )
    require(
        isinstance(raw_native, dict)
        and set(raw_native) == {"report", "sha256", "blake3", "size_bytes"}
        and isinstance(raw_native.get("report"), dict),
        "hosted native PostgreSQL raw verifier report binding is malformed",
    )
    native_facts = verify_native_restore_report(native_restore)
    require(
        raw_native.get("report") == native_restore,
        "hosted native PostgreSQL raw verifier report differs from its projection",
    )
    canonical_native = json.dumps(
        native_restore, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    restore_receipt = actions[2].get("receipt")
    require(
        isinstance(restore_receipt, dict)
        and set(restore_receipt)
        == {
            "schema",
            "status",
            "report_digest",
            "report_size_bytes",
            "native_dump_digest",
            "native_dump_size_bytes",
            "native_dump_identity",
            "report_identity",
            "recovery_record_identity",
            "recovery_record_name",
            "source_provider_identity",
            "target_provider_identity",
            "secrets_emitted",
        }
        and restore_receipt.get("schema")
        == "worldstream/postgres-native-restore-receipt/v1"
        and restore_receipt.get("status") == "committed"
        and isinstance(restore_receipt.get("report_digest"), str)
        and re.fullmatch(r"blake3:[0-9a-f]{64}", restore_receipt["report_digest"])
        is not None
        and restore_receipt.get("report_digest") == raw_native.get("blake3")
        and restore_receipt.get("report_size_bytes") == raw_native.get("size_bytes")
        and restore_receipt.get("native_dump_digest")
        == native_restore.get("native_dump_digest")
        and restore_receipt.get("native_dump_size_bytes")
        == native_restore.get("native_dump_size_bytes")
        and all(
            isinstance(restore_receipt.get(field), dict)
            and set(restore_receipt[field]) == {"storage_id", "file_id"}
            and isinstance(restore_receipt[field].get("storage_id"), str)
            and re.fullmatch(r"[0-9a-f]{16}", restore_receipt[field]["storage_id"])
            is not None
            and isinstance(restore_receipt[field].get("file_id"), str)
            and re.fullmatch(r"[0-9a-f]{32}", restore_receipt[field]["file_id"])
            is not None
            for field in (
                "native_dump_identity",
                "report_identity",
                "recovery_record_identity",
            )
        )
        and isinstance(restore_receipt.get("recovery_record_name"), str)
        and re.fullmatch(
            r"\.worldstream_native_recovery_[0-9a-f]{32}\.json",
            restore_receipt["recovery_record_name"],
        )
        is not None
        and restore_receipt.get("source_provider_identity")
        == native_restore.get("source_provider_identity")
        and snapshot_receipt.get("source_provider_identity")
        == native_restore.get("source_provider_identity")
        and restore_receipt.get("target_provider_identity")
        == native_restore.get("target_provider_identity")
        and restore_receipt.get("secrets_emitted") is False
        and isinstance(product_execution.get("native_report_sha256"), str)
        and SHA256_REF.fullmatch(product_execution["native_report_sha256"]) is not None
        and type(product_execution.get("native_report_size_bytes")) is int
        and 0 < product_execution["native_report_size_bytes"] <= 64 * 1024 * 1024
        and product_execution.get("native_report_sha256") == raw_native.get("sha256")
        and product_execution.get("native_report_size_bytes")
        == raw_native.get("size_bytes")
        and product_execution.get("native_report_canonical_sha256")
        == "sha256:" + hashlib.sha256(canonical_native).hexdigest(),
        "hosted native PostgreSQL restore receipt does not bind the committed report",
    )
    receipt_identities = [
        (
            restore_receipt[field]["storage_id"],
            restore_receipt[field]["file_id"],
        )
        for field in (
            "native_dump_identity",
            "report_identity",
            "recovery_record_identity",
        )
    ]
    require(
        len(set(receipt_identities)) == len(receipt_identities),
        "hosted native PostgreSQL receipt artifact identities must be distinct",
    )
    raw_dump = private_observations.get("native_dump")
    require(
        isinstance(raw_dump, dict)
        and set(raw_dump) == {"sha256", "blake3", "size_bytes"}
        and raw_dump.get("blake3") == "blake3:" + restore_receipt["native_dump_digest"]
        and raw_dump.get("size_bytes") == restore_receipt["native_dump_size_bytes"],
        "hosted native PostgreSQL retained dump bytes do not match the receipt",
    )
    cleanup = report.get("cleanup")
    require(
        isinstance(cleanup, dict)
        and set(cleanup)
        == {
            "status",
            "admitted_target_identity",
            "target_marker_before_drop",
            "target_connection_limit_before_drop",
            "target_backends_before_drop",
            "generated_restore_roles_before_drop",
            "target_database_after_drop",
            "provider_cleanup_transcript_sha256",
            "private_artifacts_disposition",
            "private_artifact_placeholder_count",
            "operator_passfile_disposition",
        }
        and cleanup.get("status") == "pass"
        and isinstance(cleanup.get("admitted_target_identity"), dict)
        and set(cleanup["admitted_target_identity"])
        == {"system_identifier", "database_oid", "database_name"}
        and provider_identity_is_canonical(cleanup["admitted_target_identity"])
        and cleanup["admitted_target_identity"].get("database_name")
        == target["database"]
        and cleanup["admitted_target_identity"]
        == native_restore.get("target_provider_identity")
        and cleanup.get("target_marker_before_drop")
        == "worldstream/native-postgres-disposable-target/v1"
        and type(cleanup.get("target_connection_limit_before_drop")) is int
        and cleanup["target_connection_limit_before_drop"] == 0
        and type(cleanup.get("target_backends_before_drop")) is int
        and cleanup["target_backends_before_drop"] == 0
        and type(cleanup.get("generated_restore_roles_before_drop")) is int
        and cleanup["generated_restore_roles_before_drop"] == 0
        and cleanup.get("target_database_after_drop") == "absent"
        and isinstance(cleanup.get("provider_cleanup_transcript_sha256"), str)
        and SHA256_REF.fullmatch(cleanup["provider_cleanup_transcript_sha256"])
        is not None
        and cleanup.get("private_artifacts_disposition")
        == "exact_retained_root_scrubbed_to_zero_length_placeholders"
        and type(cleanup.get("private_artifact_placeholder_count")) is int
        and 0 < cleanup["private_artifact_placeholder_count"] <= 16
        and cleanup.get("operator_passfile_disposition")
        == "exact_retained_file_scrubbed_to_zero_length",
        "hosted native PostgreSQL cleanup was not directly observed",
    )
    execution_bytes = json.dumps(
        product_execution, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    tools_bytes = json.dumps(
        provider_tools, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    cleanup_bytes = json.dumps(
        cleanup, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return {
        **native_facts,
        "fixture_classification": UNTRUSTED_FIXTURE_CLASSIFICATION,
        "fixture_report_sha256": expected_fixture["fixture_report_sha256"],
        "fixture_report_size_bytes": expected_fixture["fixture_report_size_bytes"],
        "source_revision": runtime_binding["source_revision"],
        "packaged_control_sha256": runtime_binding["worldstreamctl_sha256"],
        "packaged_control_size_bytes": runtime_binding["worldstreamctl_size_bytes"],
        "native_raw_report_sha256": raw_native["sha256"],
        "native_raw_report_size_bytes": raw_native["size_bytes"],
        "native_product_execution_sha256": "sha256:"
        + hashlib.sha256(execution_bytes).hexdigest(),
        "native_provider_tools_sha256": "sha256:"
        + hashlib.sha256(tools_bytes).hexdigest(),
        "native_cleanup_sha256": "sha256:" + hashlib.sha256(cleanup_bytes).hexdigest(),
    }


def emit_native(args: argparse.Namespace) -> None:
    contract = manifest()
    source_id = args.source
    expected_system = "Linux" if source_id == "native-linux" else "Windows"
    require(
        args.native_restore_report is not None
        and args.native_fixture_report is not None
        and args.native_binding_parent is not None,
        f"missing hosted {expected_system} native PostgreSQL restore inputs",
    )
    native_inputs = {
        args.native_restore_report.resolve(),
        args.native_fixture_report.resolve(),
        args.package_report.resolve(),
        args.gate_report.resolve(),
        args.runtime_report.resolve(),
        args.artifact.resolve(),
        args.native_binding_parent.resolve(),
    }
    require(
        len(native_inputs) == 7,
        f"hosted {expected_system} native PostgreSQL input was substituted by another input",
    )
    retained_inputs: dict[str, RetainedJsonInput] = {}
    retained_artifact: RetainedArtifactInput | None = None
    private_binding = PrivateNativeBinding()
    pending_outputs: list[tuple[Path, dict[str, Any]]] = []
    primary_error: BaseException | None = None
    try:
        for key, path, label in (
            ("package", args.package_report, "package report"),
            ("gate", args.gate_report, "platform gate report"),
            ("runtime", args.runtime_report, "native packaged runtime report"),
            (
                "hosted",
                args.native_restore_report,
                f"hosted {expected_system} native PostgreSQL restore report",
            ),
            (
                "fixture",
                args.native_fixture_report,
                f"hosted {expected_system} native PostgreSQL source fixture report",
            ),
        ):
            retained_inputs[key] = RetainedJsonInput.open(path, label)
        retained_artifact = RetainedArtifactInput.open(args.artifact, "release archive")
        retained_identities = {
            (value.retained.admitted.st_dev, value.retained.admitted.st_ino)
            for value in retained_inputs.values()
        }
        retained_identities.add(
            (
                retained_artifact.retained.admitted.st_dev,
                retained_artifact.retained.admitted.st_ino,
            )
        )
        require(
            len(retained_identities) == len(retained_inputs) + 1,
            f"hosted {expected_system} native PostgreSQL inputs must have distinct file identities",
        )
        package_report = retained_inputs["package"].value
        gate_report = retained_inputs["gate"].value
        runtime_report = retained_inputs["runtime"].value
        hosted_native_restore = retained_inputs["hosted"].value
        native_fixture_report = retained_inputs["fixture"].value

        private_binding.admit(
            args.native_binding_parent,
            hosted_native_restore.get("private_binding"),
        )
        raw_actions = []
        for key in (
            "artifact_directory_stdout",
            "snapshot_rebuild_stdout",
            "native_restore_stdout",
        ):
            receipt, raw = private_binding.read_json(key)
            raw_actions.append(
                {
                    "receipt": receipt,
                    "sha256": "sha256:" + hashlib.sha256(raw).hexdigest(),
                    "size_bytes": len(raw),
                }
            )
        raw_native_report, raw_native_bytes = private_binding.read_json("native_report")
        dump_sha256, dump_blake3, dump_size = private_binding.dump_hashes()
        private_observations = {
            "manifest": hosted_native_restore["private_binding"],
            "actions": raw_actions,
            "native_report": {
                "report": raw_native_report,
                "sha256": "sha256:" + hashlib.sha256(raw_native_bytes).hexdigest(),
                "blake3": "blake3:" + BLAKE3(raw_native_bytes).hex(),
                "size_bytes": len(raw_native_bytes),
            },
            "native_dump": {
                "sha256": dump_sha256,
                "blake3": dump_blake3,
                "size_bytes": dump_size,
            },
        }

        retained_artifact.verify()
        verified_package_report, archive_binaries = independently_verify_native_archive(
            args.artifact, source_id, contract["release_candidate"]
        )
        retained_artifact.verify()
        verify_archive_report(
            package_report,
            args.artifact,
            source_id,
            contract,
            verified_package_report,
            artifact_sha256=retained_artifact.sha256,
            artifact_size_bytes=retained_artifact.size_bytes,
        )
        artifact_sha256 = retained_artifact.sha256
        artifact_size_bytes = retained_artifact.size_bytes
        runtime_binding = verify_native_runtime_report(
            runtime_report,
            package_report,
            retained_inputs["package"].sha256,
            retained_inputs["package"].size_bytes,
            args.artifact,
            source_id,
            archive_binaries,
            artifact_sha256=artifact_sha256,
            artifact_size_bytes=artifact_size_bytes,
        )
        native_restore_facts = verify_hosted_native_restore_report(
            hosted_native_restore,
            source_id=source_id,
            package_report_sha256=retained_inputs["package"].sha256,
            package_report_size_bytes=retained_inputs["package"].size_bytes,
            artifact_sha256=artifact_sha256,
            artifact_size_bytes=artifact_size_bytes,
            runtime_binding=runtime_binding,
            fixture_report_sha256=retained_inputs["fixture"].sha256,
            fixture_report_size_bytes=retained_inputs["fixture"].size_bytes,
            fixture_report=native_fixture_report,
            private_observations=private_observations,
        )
        runtime_required = {
            "source-version-drift",
            "compatibility-manifest-verify",
            "rust-tests",
            "sqlite-critical-matrix",
            "postgres-local",
            "evidence-backup-restore-local",
            "evidence-privacy-local",
            "evidence-capability-local",
            "evidence-lease-local",
            "evidence-telemetry-local",
            "ci-cell",
        }
        if source_id == "native-linux":
            runtime_required |= {
                "postgres-live-contract",
                "process-kill-point",
                "telemetry-failure-pressure",
                "bounded-sqlite-soak",
            }
            filesystem_required = {
                "filesystem-owner-only",
                "filesystem-link-policy",
                "evidence-filesystem-local",
            }
        else:
            filesystem_required = {
                "filesystem-owner-only",
                "filesystem-acl-policy",
                "evidence-filesystem-local",
            }
        outcomes = gate_outcomes(
            gate_report, expected_system, runtime_required | filesystem_required
        )
        cell_detail = outcomes["ci-cell"].get("detail", "")
        require(
            source_id.removeprefix("native-") in cell_detail,
            "gate cell identity mismatch",
        )
        for retained in retained_inputs.values():
            retained.verify()
        retained_artifact.verify()

        output = args.output_dir
        pending_outputs.append(
            (
                output / f"{source_id}-package.json",
                diagnostic(
                    source_id,
                    "package",
                    {
                        "archive_identity": package_report["sha256"],
                        "release_candidate": contract["release_candidate"],
                        "platform_identity": PLATFORMS[source_id],
                        "artifact_path": args.artifact.name,
                    },
                    contract,
                ),
            )
        )
        pending_outputs.append(
            (
                output / f"{source_id}-runtime.json",
                diagnostic(
                    source_id,
                    "runtime",
                    {
                        "platform_identity": PLATFORMS[source_id],
                        "release_candidate": contract["release_candidate"],
                        "runtime_smoke": ",".join(sorted(runtime_required)),
                        "storage_profile": "sqlite-bundled,postgres-primary",
                        "runtime_report_sha256": retained_inputs["runtime"].sha256,
                        "packaged_binary_sha256": runtime_binding[
                            "worldstreamd_sha256"
                        ],
                        "packaged_control_sha256": runtime_binding[
                            "worldstreamctl_sha256"
                        ],
                        "postgres_admin": "packaged ctl migrate/verify and least-privilege file-only runtime passed",
                    },
                    contract,
                ),
            )
        )
        filesystem_kind = (
            "filesystem" if source_id == "native-linux" else "acl-or-reparse"
        )
        filesystem_facts = (
            {
                "platform_identity": PLATFORMS[source_id],
                "filesystem_policy": "owner-only and local-filesystem policy passed",
                "owner_only_paths": "data and secret paths verified",
                "symlink_rejection": "symlink policy probe passed",
            }
            if source_id == "native-linux"
            else {
                "platform_identity": PLATFORMS[source_id],
                "acl_policy": "protected owner/SYSTEM/Administrators DACL tests passed",
                "reparse_policy": "nonlocal and ambiguous Windows path tests passed",
                "broad_write_rejected": "broad directory and secret-file DACLs rejected",
            }
        )
        pending_outputs.append(
            (
                output / f"{source_id}-{filesystem_kind}.json",
                diagnostic(source_id, filesystem_kind, filesystem_facts, contract),
            )
        )
        pending_outputs.append(
            (
                output / f"{source_id}-native-postgres-restore.json",
                diagnostic(
                    source_id,
                    "native-postgres-restore",
                    {
                        "platform_identity": PLATFORMS[source_id],
                        "release_candidate": contract["release_candidate"],
                        "native_restore_report_sha256": retained_inputs[
                            "hosted"
                        ].sha256,
                        "native_restore_report_size_bytes": retained_inputs[
                            "hosted"
                        ].size_bytes,
                        "release_archive_sha256": package_report["sha256"],
                        "release_archive_size_bytes": artifact_size_bytes,
                        "package_report_sha256": retained_inputs["package"].sha256,
                        "package_report_size_bytes": retained_inputs[
                            "package"
                        ].size_bytes,
                        "packaged_runtime_report_sha256": retained_inputs[
                            "runtime"
                        ].sha256,
                        "packaged_runtime_report_size_bytes": retained_inputs[
                            "runtime"
                        ].size_bytes,
                        **native_restore_facts,
                    },
                    contract,
                ),
            )
        )
    except BaseException as error:  # noqa: BLE001 - preserve cleanup on interrupts
        primary_error = error
    cleanup_errors: list[BaseException] = []
    try:
        private_binding.scrub_exact()
    except BaseException as error:  # noqa: BLE001 - cleanup must continue
        cleanup_errors.append(error)
    try:
        private_binding.close()
    except BaseException as error:  # noqa: BLE001 - cleanup must continue
        cleanup_errors.append(error)
    retained_resources: list[Any] = list(retained_inputs.values())
    if retained_artifact is not None:
        retained_resources.append(retained_artifact)
    cleanup_errors.extend(_attempt_retained_closes(retained_resources))
    _finish_retained_scope(
        primary_error=primary_error,
        cleanup_errors=cleanup_errors,
        label="native diagnostic retained authority",
    )
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for path, value in pending_outputs:
        atomic_write(path, value)


def _build_oci_output(
    args: argparse.Namespace,
    contract: dict[str, Any],
    retained_inputs: dict[str, RetainedJsonInput],
    retained_artifact: RetainedArtifactInput,
) -> dict[str, Any]:
    context_input = retained_inputs["context"]
    runtime_input = retained_inputs["runtime"]
    supplied_metadata = retained_inputs["supplied_metadata"]
    verified_metadata = retained_inputs["verified_metadata"]
    context = context_input.value
    runtime = runtime_input.value
    metadata = supplied_metadata.value
    verified_context = independently_verify_oci_context(args.context)
    require(
        context == verified_context,
        "OCI context report is not the exact independently verified context projection",
    )
    expected_metadata = args.context / "oci-metadata.json"
    supplied_metadata.verify()
    verified_metadata.verify()
    require(
        args.context_metadata.resolve() == expected_metadata.resolve()
        and supplied_metadata.raw == verified_metadata.raw,
        "OCI context metadata is not the exact metadata inside the verified context",
    )
    runtime_binding = runtime.get("artifact_binding")
    tested_image_config_digest = (
        runtime_binding.get("tested_image_config_digest")
        if isinstance(runtime_binding, dict)
        else None
    )
    require(
        isinstance(tested_image_config_digest, str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", tested_image_config_digest)
        is not None,
        "OCI runtime diagnostic has no tested image config digest",
    )
    retained_artifact.verify()
    verified_artifact = independently_verify_oci_archive(
        args.artifact, tested_image_config_digest
    )
    retained_artifact.verify()
    require(
        runtime_binding == verified_artifact
        and verified_artifact.get("artifact_sha256") == retained_artifact.sha256
        and verified_artifact.get("artifact_size_bytes")
        == retained_artifact.size_bytes,
        "OCI runtime artifact binding is not the exact independently verified archive projection",
    )
    artifact_digest = retained_artifact.sha256
    profiles = runtime.get("profiles")
    secret_scan = runtime.get("secret_scan")
    sqlite_volume = runtime.get("sqlite_volume")
    sqlite = contract.get("storage", {}).get("sqlite", {})
    sqlite_identity = (
        f"sqlite/{sqlite.get('version')}; source_id={sqlite.get('source_id')}"
    )
    require(
        isinstance(secret_scan, dict)
        and set(secret_scan)
        == {
            "schema",
            "status",
            "secrets_emitted",
            "sentinel_sha256",
            "encodings_scanned",
            "channels",
        }
        and secret_scan.get("schema") == "worldstream/secret-absence-scan/v1"
        and secret_scan.get("status") == "pass"
        and secret_scan.get("secrets_emitted") is False
        and isinstance(secret_scan.get("sentinel_sha256"), str)
        and re.fullmatch(r"sha256:[0-9a-f]{64}", secret_scan["sentinel_sha256"])
        is not None
        and secret_scan.get("encodings_scanned")
        == ["base64", "base64url", "hex", "raw"]
        and isinstance(secret_scan.get("channels"), list)
        and len(secret_scan["channels"]) == len(OCI_SECRET_SCAN_CHANNELS),
        "OCI secret-absence scan is incomplete",
    )
    observed_secret_channels: set[str] = set()
    for channel in secret_scan["channels"]:
        require(
            isinstance(channel, dict)
            and set(channel) == {"channel", "sha256", "size_bytes"}
            and isinstance(channel.get("channel"), str)
            and channel["channel"] not in observed_secret_channels
            and isinstance(channel.get("sha256"), str)
            and re.fullmatch(r"sha256:[0-9a-f]{64}", channel["sha256"]) is not None
            and type(channel.get("size_bytes")) is int
            and channel["size_bytes"] >= 0,
            "OCI secret-absence scan channel is malformed",
        )
        observed_secret_channels.add(channel["channel"])
    require(
        observed_secret_channels == OCI_SECRET_SCAN_CHANNELS,
        "OCI secret-absence scan does not cover the exact channel set",
    )
    require(
        isinstance(sqlite_volume, dict)
        and set(sqlite_volume)
        == {
            "path",
            "type",
            "driver",
            "scope",
            "driver_options",
            "mount_device",
            "filesystem",
            "mount_source",
            "locality",
        }
        and sqlite_volume.get("path") == "/var/lib/worldstream"
        and sqlite_volume.get("type") == "docker-volume"
        and sqlite_volume.get("driver") == "local"
        and sqlite_volume.get("scope") == "local"
        and sqlite_volume.get("driver_options") == {}
        and isinstance(sqlite_volume.get("mount_device"), str)
        and re.fullmatch(r"[1-9][0-9]*:[0-9]+", sqlite_volume["mount_device"])
        is not None
        and sqlite_volume.get("filesystem") in {"ext4", "xfs"}
        and isinstance(sqlite_volume.get("mount_source"), str)
        and re.fullmatch(r"/dev/[^\s]+", sqlite_volume["mount_source"]) is not None
        and sqlite_volume.get("locality") == "local-block-device",
        "OCI SQLite volume locality is incomplete",
    )
    require(
        context.get("schema") == "worldstream/oci-context-report/v1"
        and context.get("kind") == "oci-context"
        and context.get("verified") is True
        and context.get("release_evidence") is False,
        "OCI context was not exactly verified",
    )
    require(
        runtime.get("schema") == "worldstream/oci-runtime-smoke/v1"
        and runtime.get("status") == "PASS"
        and runtime.get("release_evidence") is False
        and runtime.get("health") == "healthy"
        and runtime.get("healthcheck_without_daemon") == "rejected"
        and runtime.get("read_only_root") is True
        and runtime.get("non_root") == "65532:65532"
        and runtime.get("persistent_volume") == "/var/lib/worldstream"
        and runtime.get("authority_secret_source")
        == "owner-readable-read-only-volume-file"
        and runtime.get("standalone_config") == "valid"
        and runtime.get("rejected_layouts")
        == ["tmpfs", "wrong-data-directory", "network-configured-volume"]
        and runtime.get("secrets_emitted") is False
        and secret_scan.get("secrets_emitted") is False
        and isinstance(profiles, dict)
        and set(profiles) == {"sqlite-bundled", "postgres-primary"}
        and profiles.get("sqlite-bundled")
        == {
            "status": "pass",
            "health": "healthy",
            "filesystem": "ext4-or-xfs-explicit-volume",
            "engine_identity": sqlite_identity,
            "healthz": "pass",
            "readyz": "pass",
            "version": "pass",
        }
        and profiles.get("postgres-primary")
        == {
            "status": "pass",
            "provider_image": OCI_POSTGRES_IMAGE,
            "engine_identity": OCI_POSTGRES_IDENTITY,
            "packaged_admin_migrate": "pass",
            "packaged_admin_verify": "pass",
            "runtime_role_least_privilege": True,
            "healthz": "pass",
            "readyz": "pass",
            "version": "pass",
            "network": "disabled-unix-socket",
        }
        and runtime_binding == verified_artifact,
        "OCI runtime diagnostic did not pass the frozen profile",
    )
    base_image = metadata.get("base_image")
    require(
        isinstance(base_image, str)
        and re.fullmatch(r"[^\s@]+@sha256:[0-9a-f]{64}", base_image) is not None
        and metadata.get("version") == contract["release_candidate"]
        and metadata.get("profile") == "oci-linux-amd64"
        and metadata.get("target") == "linux/amd64",
        "OCI metadata identity or pinned base image is invalid",
    )
    return diagnostic(
        "oci-linux",
        "oci",
        {
            "platform_identity": "oci-linux-amd64",
            "release_candidate": contract["release_candidate"],
            "pinned_base_image": base_image,
            "image_digest": artifact_digest,
            "tested_image_config_digest": tested_image_config_digest,
            "context_inventory_sha256": verified_context["sha256"],
            "context_report_sha256": context_input.sha256,
            "context_metadata_sha256": verified_metadata.sha256,
            "runtime_report_sha256": runtime_input.sha256,
            "runtime_smoke": "healthy non-root read-only runtime with verified local persistent volume",
            "storage_profiles": "sqlite-bundled and postgres-primary passed",
            "postgres_provider_image": OCI_POSTGRES_IMAGE,
            "postgres_engine_identity": OCI_POSTGRES_IDENTITY,
            "postgres_packaged_administration": "migrate and verify passed",
            "postgres_runtime_role": "least privilege verified",
            "postgres_network": "disabled network with local Unix socket",
            "secrets": "not emitted",
            "filesystem_policy": "ext4/xfs local block source, local driver without options, and negative layout probes passed",
        },
        contract,
    )


def emit_oci(args: argparse.Namespace) -> None:
    contract = manifest()
    retained_inputs: dict[str, RetainedJsonInput] = {}
    retained_artifact: RetainedArtifactInput | None = None
    pending_output: dict[str, Any] | None = None
    primary_error: BaseException | None = None
    try:
        expected_metadata = args.context / "oci-metadata.json"
        for key, path, label in (
            ("context", args.context_report, "OCI context report"),
            ("runtime", args.runtime_report, "OCI runtime report"),
            ("supplied_metadata", args.context_metadata, "OCI context metadata"),
            (
                "verified_metadata",
                expected_metadata,
                "verified OCI context metadata",
            ),
        ):
            retained_inputs[key] = RetainedJsonInput.open(path, label)
        retained_artifact = RetainedArtifactInput.open(
            args.artifact, "OCI release archive"
        )
        pending_output = _build_oci_output(
            args, contract, retained_inputs, retained_artifact
        )
        for retained in retained_inputs.values():
            retained.verify()
        retained_artifact.verify()
    except BaseException as error:  # noqa: BLE001 - close retained input on interrupts
        primary_error = error
    retained_resources: list[Any] = list(retained_inputs.values())
    if retained_artifact is not None:
        retained_resources.append(retained_artifact)
    cleanup_errors = _attempt_retained_closes(retained_resources)
    _finish_retained_scope(
        primary_error=primary_error,
        cleanup_errors=cleanup_errors,
        label="OCI diagnostic retained authority",
    )
    assert pending_output is not None
    atomic_write(args.output_dir / "oci-linux-oci.json", pending_output)


def verify_macos_browser_story(
    value: Any,
    architecture: str,
    *,
    adapter_sha256: str,
    adapter_size_bytes: int,
) -> None:
    expected_checks = {
        "browser_identity_verified": True,
        "catch_up_or_reset_installed": True,
        "embedded_ui_loaded": True,
        "final_reveal_dom_visible": True,
        "new_session_resynchronized": True,
        "package_bound_reference_clients": False,
        "package_bound_runtime": False,
        "precomplete_reveal_locked": True,
        "privacy_negative_dom_and_browser_channels": True,
        "replay_hashes_verified": True,
        "six_phase_story_complete": True,
        "stale_head_rejected": True,
        "typed_actions_accepted_in_dom": True,
    }
    require(
        isinstance(value, dict)
        and set(value)
        == {
            "schema",
            "canonical_encoding",
            "status",
            "release_evidence",
            "source_mode",
            "elapsed_ms",
            "browser",
            "tools",
            "runtime",
            "story",
            "dom_evidence",
            "typed_actions",
            "checks",
            "privacy",
        }
        and value.get("schema") == "worldstream/package-browser-heist/v1"
        and value.get("canonical_encoding") == "utf8-sorted-key-compact-json-lf"
        and value.get("status") == "pass"
        and value.get("release_evidence") is False
        and value.get("source_mode") == "source-build"
        and type(value.get("elapsed_ms")) is int
        and 0 < value["elapsed_ms"] <= 600_000
        and value.get("browser") == MACOS_BROWSER_IDENTITIES[architecture]
        and value.get("checks") == expected_checks
        and value.get("privacy")
        == {
            "status": "pass",
            "private_canary_absent": True,
            "credentials_absent": True,
            "private_claim_absent_from_retained_evidence": True,
        },
        "macOS source quickstart browser story contract is incomplete",
    )
    dom = value["dom_evidence"]
    actions = value["typed_actions"]
    require(
        isinstance(dom, dict)
        and set(dom)
        == {
            "stale_rejection",
            "precomplete_reveal",
            "public_final",
            "participant_final",
            "operator_final",
            "replay_final",
            "briefing",
            "negotiation",
            "commitment",
            "result",
            "complete",
            "resync",
            "browser_diagnostics",
        }
        and all(re.fullmatch(r"sha256:[0-9a-f]{64}", item) for item in dom.values())
        and isinstance(actions, dict)
        and set(actions)
        == {
            "inspect_clue",
            "publish_clue",
            "propose_plan",
            "commit_move",
            "acknowledge_result",
        }
        and all(
            re.fullmatch(r"sha256:[0-9a-f]{64}", item) for item in actions.values()
        ),
        "macOS source quickstart DOM/action evidence is incomplete",
    )
    story = value["story"]
    public_projection = (
        story.get("public_projection") if isinstance(story, dict) else None
    )
    final_replay = story.get("final_replay") if isinstance(story, dict) else None
    hash_parity = (
        final_replay.get("hash_parity") if isinstance(final_replay, dict) else None
    )
    replay_fields = [
        "pack",
        "core",
        "activity",
        "aggregate_authoritative",
        "transition",
        "room_id",
        "room_seq",
    ]
    replay_hash_fields = {
        "pack",
        "core",
        "activity",
        "aggregate_authoritative",
        "transition",
    }
    require(
        isinstance(story, dict)
        and set(story) == {"phase_path", "public_projection", "final_replay"}
        and story.get("phase_path")
        == ["Briefing", "Negotiation", "Commitment", "Resolution", "Result", "Complete"]
        and isinstance(public_projection, dict)
        and set(public_projection)
        == {
            "broker_present",
            "public_claims",
            "plans",
            "endorsements",
            "challenges",
            "commitment_count",
            "aggregate_outcome_present",
        }
        and public_projection.get("broker_present") is True
        and all(
            type(public_projection.get(field)) is int and public_projection[field] >= 0
            for field in (
                "public_claims",
                "plans",
                "endorsements",
                "challenges",
            )
        )
        and type(public_projection.get("commitment_count")) is int
        and public_projection["commitment_count"] == 2
        and public_projection.get("aggregate_outcome_present") is True
        and isinstance(final_replay, dict)
        and set(final_replay) == {"verified", "hash_parity"}
        and final_replay.get("verified") is True
        and isinstance(hash_parity, dict)
        and set(hash_parity) == {"verified", "fields", "expected", "replayed"}
        and hash_parity.get("verified") is True
        and hash_parity.get("fields") == replay_fields
        and isinstance(hash_parity.get("expected"), dict)
        and set(hash_parity["expected"]) == replay_hash_fields
        and hash_parity.get("replayed") == hash_parity["expected"]
        and all(
            isinstance(item, str) and 0 < len(item) <= 256
            for item in hash_parity["expected"].values()
        ),
        "macOS source quickstart story/replay evidence is incomplete",
    )
    runtime = value["runtime"]
    require(
        isinstance(runtime, dict)
        and set(runtime) == {"worldstreamd", "ui", "sdk", "heist_reference_clients"}
        and all(
            isinstance(runtime.get(name), dict)
            for name in ("worldstreamd", "ui", "sdk", "heist_reference_clients")
        )
        and set(runtime.get("worldstreamd", {})) == {"origin", "sha256", "size_bytes"}
        and set(runtime.get("ui", {}))
        == {"origin", "tree_sha256", "index_sha256", "file_count", "total_bytes"}
        and set(runtime.get("sdk", {}))
        == {"origin", "tree_sha256", "file_count", "total_bytes"}
        and set(runtime.get("heist_reference_clients", {}))
        == {"origin", "tree_sha256", "file_count", "total_bytes"}
        and runtime.get("worldstreamd", {}).get("origin")
        == "source-build:target/debug/worldstreamd"
        and runtime.get("ui", {}).get("origin") == "source-build:web/console/dist"
        and runtime.get("sdk", {}).get("origin") == "source:sdk/python/src"
        and runtime.get("heist_reference_clients", {}).get("origin")
        == "source:examples/heist"
        and all(
            re.fullmatch(r"sha256:[0-9a-f]{64}", str(record.get("sha256")))
            and type(record.get("size_bytes")) is int
            and record["size_bytes"] > 0
            for record in (runtime["worldstreamd"],)
        )
        and all(
            re.fullmatch(r"sha256:[0-9a-f]{64}", str(record.get("tree_sha256")))
            and type(record.get("file_count")) is int
            and record["file_count"] > 0
            and type(record.get("total_bytes")) is int
            and record["total_bytes"] > 0
            for record in (
                runtime["ui"],
                runtime["sdk"],
                runtime["heist_reference_clients"],
            )
        ),
        "macOS source quickstart runtime identity is incomplete",
    )
    tools = value["tools"]
    require(
        isinstance(tools, dict)
        and set(tools) == {"adapter", "python"}
        and isinstance(tools.get("adapter"), dict)
        and set(tools["adapter"]) == {"name", "protocol", "sha256", "size_bytes"}
        and tools.get("adapter")
        == {
            "name": "worldstream-cdp-browser",
            "protocol": "Chrome DevTools Protocol",
            "sha256": adapter_sha256,
            "size_bytes": adapter_size_bytes,
        }
        and tools.get("python")
        == {
            "implementation": "cpython",
            "version": pinned_macos_toolchains()["python"],
        },
        "macOS source quickstart browser tool identity is incomplete",
    )


def macos_quickstart_projection(quickstart: dict[str, Any]) -> dict[str, Any]:
    """Return the exact public allowlist; omit host OS version and unknown claims."""

    return {
        "schema": quickstart["schema"],
        "status": quickstart["status"],
        "release_evidence": quickstart["release_evidence"],
        "signed_or_notarized_binary": quickstart["signed_or_notarized_binary"],
        "version": quickstart["version"],
        "platform": {
            key: quickstart["platform"][key]
            for key in ("system", "filesystem", "machine")
        },
        "toolchains": quickstart["toolchains"],
        "source_revision": quickstart["source_revision"],
        "elapsed_seconds": quickstart["elapsed_seconds"],
        "browser_story": quickstart["browser_story"],
        "checks": quickstart["checks"],
    }


def emit_macos(args: argparse.Namespace) -> None:
    contract = manifest()
    require(
        isinstance(args.source_revision, str)
        and re.fullmatch(r"[0-9a-f]{40}", args.source_revision) is not None,
        "macOS source diagnostic requires an exact 40-hex source revision",
    )
    expected_toolchains = pinned_macos_toolchains()
    retained: list[RetainedJsonInput] = []
    retained_adapter: RetainedArtifactInput | None = None
    quickstarts: dict[str, RetainedJsonInput] = {}
    pending_outputs: list[tuple[Path, dict[str, Any]]] = []
    primary_error: BaseException | None = None
    try:
        retained_adapter = RetainedArtifactInput.open(
            ROOT / "scripts/cdp-browser.py", "macOS browser adapter"
        )
        for report_path in args.quickstart_report:
            observed = RetainedJsonInput.open(report_path, "macOS quickstart report")
            retained.append(observed)
            quickstart = observed.value
            observed_architecture = quickstart.get("platform", {}).get("machine")
            require(
                observed_architecture in MACOS_ARCHITECTURES,
                "macOS source quickstart has an unexpected architecture",
            )
            require(
                observed_architecture not in quickstarts,
                f"duplicate macOS source quickstart architecture: {observed_architecture}",
            )
            require(
                set(quickstart)
                == {
                    "schema",
                    "status",
                    "release_evidence",
                    "signed_or_notarized_binary",
                    "version",
                    "platform",
                    "toolchains",
                    "source_revision",
                    "elapsed_seconds",
                    "browser_story",
                    "checks",
                }
                and quickstart.get("schema") == "worldstream/macos-source-quickstart/v2"
                and quickstart.get("status") == "passed"
                and quickstart.get("release_evidence") is False
                and quickstart.get("signed_or_notarized_binary") is False
                and isinstance(quickstart.get("platform"), dict)
                and set(quickstart["platform"])
                == {"system", "version", "filesystem", "machine"}
                and quickstart["platform"].get("system") == "Darwin"
                and isinstance(quickstart["platform"].get("version"), str)
                and 0 < len(quickstart["platform"]["version"]) <= 128
                and quickstart["platform"].get("filesystem") == "apfs"
                and quickstart.get("version") == contract["release_candidate"]
                and quickstart.get("source_revision") == args.source_revision
                and quickstart.get("toolchains") == expected_toolchains
                and type(quickstart.get("elapsed_seconds")) is int
                and 0 < quickstart["elapsed_seconds"] < 600
                and quickstart.get("checks")
                == {
                    "complete_heist": True,
                    "embedded_ui": True,
                    "pinned_toolchain": True,
                    "privacy": True,
                    "quickstart": True,
                    "real_browser": True,
                    "replay": True,
                    "source_build": True,
                    "source_revision": True,
                    "stale_resync": True,
                },
                "macOS source quickstart diagnostic is incomplete",
            )
            verify_macos_browser_story(
                quickstart["browser_story"],
                observed_architecture,
                adapter_sha256=retained_adapter.sha256,
                adapter_size_bytes=retained_adapter.size_bytes,
            )
            quickstarts[observed_architecture] = observed
        require(
            set(quickstarts) == set(MACOS_ARCHITECTURES),
            "macOS source quickstart requires exactly arm64 and x86_64 reports",
        )
        require(
            len(
                {
                    (item.retained.admitted.st_dev, item.retained.admitted.st_ino)
                    for item in retained
                }
            )
            == len(retained),
            "macOS source quickstart reports must have distinct identities",
        )
        for item in retained:
            item.verify()
        artifact = {
            "schema": "worldstream/macos-source-matrix/v1",
            "status": "passed",
            "release_evidence": False,
            "signed_or_notarized_binary": False,
            "version": contract["release_candidate"],
            "source_revision": args.source_revision,
            "toolchains": expected_toolchains,
            "architectures": list(MACOS_ARCHITECTURES),
            "quickstart_reports": [
                {
                    "architecture": architecture,
                    "sha256": quickstarts[architecture].sha256,
                    "report": macos_quickstart_projection(
                        quickstarts[architecture].value
                    ),
                }
                for architecture in MACOS_ARCHITECTURES
            ],
        }
        pending_outputs = [
            (args.artifact_output, artifact),
            (
                args.output_dir / "macos-source-source-quickstart.json",
                diagnostic(
                    "macos-source",
                    "source-quickstart",
                    {
                        "platform_identity": "macos-source",
                        "release_candidate": contract["release_candidate"],
                        "architectures": list(MACOS_ARCHITECTURES),
                        "source_revision": args.source_revision,
                        "pinned_toolchain": expected_toolchains,
                        "source_build": "workspace, Python SDK, and UI source builds passed on arm64 and x86_64",
                        "quickstart": "pinned real browser completed the six-phase Heist reference-client story through the source-built daemon and embedded UI, including stale/resync, privacy, replay, and final reveal, in under ten minutes on arm64 and x86_64",
                    },
                    contract,
                ),
            ),
        ]
        for item in retained:
            item.verify()
        retained_adapter.verify()
    except BaseException as error:  # noqa: BLE001 - close retained input on interrupts
        primary_error = error
    retained_resources: list[Any] = list(retained)
    if retained_adapter is not None:
        retained_resources.append(retained_adapter)
    cleanup_errors = _attempt_retained_closes(retained_resources)
    _finish_retained_scope(
        primary_error=primary_error,
        cleanup_errors=cleanup_errors,
        label="macOS diagnostic retained authority",
    )
    for path, value in pending_outputs:
        atomic_write(path, value)


def _build_security_outputs(
    args: argparse.Namespace,
    contract: dict[str, Any],
    retained_inputs: dict[str, RetainedJsonInput],
) -> list[tuple[Path, dict[str, Any]]]:
    linux_input = retained_inputs["linux_gate"]
    windows_input = retained_inputs["windows_gate"]
    linux_runtime_input = retained_inputs["linux_runtime"]
    windows_runtime_input = retained_inputs["windows_runtime"]
    https_input = retained_inputs["https"]
    linux = linux_input.value
    windows = windows_input.value
    linux_runtime = linux_runtime_input.value
    windows_runtime = windows_runtime_input.value
    https = https_input.value
    required = {
        "secret-scan",
        "evidence-privacy-local",
        "evidence-capability-local",
        "evidence-lease-local",
        "evidence-telemetry-local",
        "filesystem-owner-only",
        "evidence-filesystem-local",
        "config-contract",
    }
    linux_outcomes = gate_outcomes(
        linux, "Linux", required | {"telemetry-failure-pressure"}
    )
    windows_outcomes = gate_outcomes(
        windows, "Windows", required | {"filesystem-acl-policy"}
    )
    for expected_system, runtime in (
        ("Linux", linux_runtime),
        ("Windows", windows_runtime),
    ):
        profiles = runtime.get("profiles")
        require(
            runtime.get("schema") == "worldstream/native-package-runtime-smoke/v1"
            and runtime.get("status") == "pass"
            and runtime.get("release_evidence") is False
            and runtime.get("platform", {}).get("system") == expected_system
            and isinstance(profiles, dict)
            and set(profiles) == {"sqlite-bundled", "postgres-primary"}
            and all(
                profile.get("packaged_ctl", {}).get("config_validate") == "pass"
                and profile.get("packaged_ctl", {}).get("config_effective")
                == "redacted_and_precedence_exact"
                and profile.get("packaged_ctl", {}).get("doctor")
                == "bounded_diagnostics_exposed"
                for profile in profiles.values()
                if isinstance(profile, dict)
            )
            and all(isinstance(profile, dict) for profile in profiles.values()),
            f"{expected_system} packaged config/effective/doctor contract is incomplete",
        )
    require(
        set(https)
        == {
            "schema",
            "status",
            "release_evidence",
            "evidence_class",
            "platform",
            "timeout_seconds_per_test",
            "checks",
            "tests",
        },
        "telemetry HTTPS report has wrong fields",
    )
    https_platform = https.get("platform")
    require(
        https.get("schema") == "worldstream/telemetry-https-evidence/v1"
        and https.get("status") == "passed"
        and https.get("release_evidence") is False
        and https.get("evidence_class") == "local_transport_integration"
        and isinstance(https_platform, dict)
        and https_platform.get("system") == "Linux"
        and https_platform.get("machine") in {"x86_64", "amd64"}
        and type(https.get("timeout_seconds_per_test")) is int
        and 1 <= https["timeout_seconds_per_test"] <= 900
        and https.get("checks") == TELEMETRY_HTTPS_CHECKS,
        "telemetry HTTPS transport identity or checks are incomplete",
    )
    tests = https.get("tests")
    require(
        isinstance(tests, list) and len(tests) == len(TELEMETRY_HTTPS_TESTS),
        "telemetry HTTPS exact test results are missing",
    )
    observed_tests: dict[str, tuple[str, str]] = {}
    for test in tests:
        require(
            isinstance(test, dict)
            and set(test) == {"id", "package", "filter", "passed_count"}
            and isinstance(test.get("id"), str)
            and test.get("passed_count") == 1,
            "telemetry HTTPS test result is malformed or did not pass exactly once",
        )
        test_id = test["id"]
        require(test_id not in observed_tests, "duplicate telemetry HTTPS test result")
        observed_tests[test_id] = (test.get("package"), test.get("filter"))
    require(
        observed_tests == TELEMETRY_HTTPS_TESTS,
        "telemetry HTTPS exact test identity mismatch",
    )
    inputs = [
        {
            "platform": "native-linux-x86_64",
            "sha256": linux_input.sha256,
            "size_bytes": linux_input.size_bytes,
        },
        {
            "platform": "native-windows-x64",
            "sha256": windows_input.sha256,
            "size_bytes": windows_input.size_bytes,
        },
        {
            "platform": "native-linux-x86_64/telemetry-https",
            "sha256": https_input.sha256,
            "size_bytes": https_input.size_bytes,
        },
        {
            "platform": "native-linux-x86_64/config-contract",
            "sha256": linux_runtime_input.sha256,
            "size_bytes": linux_runtime_input.size_bytes,
        },
        {
            "platform": "native-windows-x64/config-contract",
            "sha256": windows_runtime_input.sha256,
            "size_bytes": windows_runtime_input.size_bytes,
        },
    ]
    artifact = {
        "schema": "worldstream/security-observability-profile/v1",
        "status": "passed",
        "release_evidence": True,
        "version": contract["release_candidate"],
        "contract": contract["contracts"],
        "platform_reports": inputs,
        "verified_outcomes": sorted(
            required | {"telemetry-failure-pressure", "filesystem-acl-policy"}
        ),
    }
    config_diagnostic = diagnostic(
        "security-observability",
        "config-redaction-observability",
        {
            "platform_identity": "all-supported-platforms",
            "config_validation": {
                "gate": "config-contract",
                "linux_gate": linux_outcomes["config-contract"]["detail"],
                "windows_gate": windows_outcomes["config-contract"]["detail"],
                "linux_runtime_sha256": linux_runtime_input.sha256,
                "windows_runtime_sha256": windows_runtime_input.sha256,
                "packaged_commands": [
                    "config validate",
                    "config effective",
                    "doctor",
                ],
            },
            "secret_redaction": "secret-scan and privacy fixtures passed on Linux and Windows",
            "observability_bounds": "telemetry fixtures and Linux failure-pressure probe passed",
        },
        contract,
    )
    security_diagnostic = diagnostic(
        "security-observability",
        "security",
        {
            "platform_identity": "all-supported-platforms",
            "security_probes": "capability, lease, privacy, and secret probes passed",
            "remote_tls": "trusted-CA and hostname-verified OTLP HTTPS delivery passed; untrusted CA, hostname mismatch, invalid endpoints, non-success, oversized, and slow responses were rejected or bounded",
            "filesystem_policy": "native POSIX and Windows DACL policies passed",
        },
        contract,
    )
    return [
        (args.artifact_output, artifact),
        (
            args.output_dir
            / "security-observability-config-redaction-observability.json",
            config_diagnostic,
        ),
        (
            args.output_dir / "security-observability-security.json",
            security_diagnostic,
        ),
    ]


def emit_security(args: argparse.Namespace) -> None:
    contract = manifest()
    retained_inputs: dict[str, RetainedJsonInput] = {}
    pending_outputs: list[tuple[Path, dict[str, Any]]] = []
    primary_error: BaseException | None = None
    try:
        for key, path, label in (
            ("linux_gate", args.linux_gate_report, "Linux platform gate report"),
            (
                "windows_gate",
                args.windows_gate_report,
                "Windows platform gate report",
            ),
            (
                "linux_runtime",
                args.linux_runtime_report,
                "Linux native packaged runtime report",
            ),
            (
                "windows_runtime",
                args.windows_runtime_report,
                "Windows native packaged runtime report",
            ),
            ("https", args.https_report, "telemetry HTTPS report"),
        ):
            retained_inputs[key] = RetainedJsonInput.open(path, label)
        pending_outputs = _build_security_outputs(args, contract, retained_inputs)
        for retained in retained_inputs.values():
            retained.verify()
    except BaseException as error:  # noqa: BLE001 - close retained input on interrupts
        primary_error = error
    cleanup_errors = _attempt_retained_closes(list(retained_inputs.values()))
    _finish_retained_scope(
        primary_error=primary_error,
        cleanup_errors=cleanup_errors,
        label="security diagnostic retained authority",
    )
    for path, value in pending_outputs:
        atomic_write(path, value)


def parser() -> argparse.ArgumentParser:
    command = argparse.ArgumentParser(description=__doc__)
    subcommands = command.add_subparsers(dest="command", required=True)
    native = subcommands.add_parser("native")
    native.add_argument(
        "--source", choices=("native-linux", "native-windows"), required=True
    )
    native.add_argument("--package-report", type=Path, required=True)
    native.add_argument("--gate-report", type=Path, required=True)
    native.add_argument("--runtime-report", type=Path, required=True)
    native.add_argument("--native-restore-report", type=Path, required=True)
    native.add_argument("--native-fixture-report", type=Path, required=True)
    native.add_argument("--native-binding-parent", type=Path, required=True)
    native.add_argument("--artifact", type=Path, required=True)
    native.add_argument("--output-dir", type=Path, required=True)
    native.set_defaults(function=emit_native)
    oci = subcommands.add_parser("oci")
    oci.add_argument("--context", type=Path, required=True)
    oci.add_argument("--context-report", type=Path, required=True)
    oci.add_argument("--runtime-report", type=Path, required=True)
    oci.add_argument("--context-metadata", type=Path, required=True)
    oci.add_argument("--artifact", type=Path, required=True)
    oci.add_argument("--output-dir", type=Path, required=True)
    oci.set_defaults(function=emit_oci)
    macos = subcommands.add_parser("macos")
    macos.add_argument("--quickstart-report", type=Path, action="append", required=True)
    macos.add_argument("--source-revision", required=True)
    macos.add_argument("--artifact-output", type=Path, required=True)
    macos.add_argument("--output-dir", type=Path, required=True)
    macos.set_defaults(function=emit_macos)
    security = subcommands.add_parser("security")
    security.add_argument("--linux-gate-report", type=Path, required=True)
    security.add_argument("--windows-gate-report", type=Path, required=True)
    security.add_argument("--linux-runtime-report", type=Path, required=True)
    security.add_argument("--windows-runtime-report", type=Path, required=True)
    security.add_argument("--https-report", type=Path, required=True)
    security.add_argument("--artifact-output", type=Path, required=True)
    security.add_argument("--output-dir", type=Path, required=True)
    security.set_defaults(function=emit_security)
    return command


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    try:
        args.function(args)
    except (DiagnosticError, KeyError, TypeError, ValueError) as error:
        print(f"platform diagnostic failed: {error}", file=sys.stderr)
        return 1
    print(f"platform diagnostic emitted by {platform.system()} verifier")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

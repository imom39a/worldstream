"""Versioned closed release-inventory contracts.

The historical inventory predates an on-wire discriminator.  Callers therefore
use ``None`` for that exact legacy behavior.  Every successor must name its
inventory explicitly in each signed graph layer.
"""

from __future__ import annotations

from dataclasses import dataclass

LEGACY_RELEASE_INVENTORY = "worldstream/release-inventory/runtime-packs-studio-v1"
CLI_FIRST_RELEASE_INVENTORY = "worldstream/release-inventory/cli-first-v1"

RUNTIME_PAYLOAD_ARTIFACT_IDS = (
    "source-archive",
    "native-linux-x86_64-archive",
    "native-windows-x64-archive",
    "oci-linux-amd64-image",
)

LEGACY_PORTABLE_SUBJECT_ARTIFACT_IDS = (
    "worldstream-a202-adapter",
    "worldstream-deterministic-agents",
    "worldstream-documentation",
    "worldstream-examples",
    "worldstream-licenses",
    "worldstream-negotiate-bundle",
    "worldstream-negotiate-evidence-verifier",
    "worldstream-pack-toolchain",
    "worldstream-participant-console",
    "worldstream-release-metadata",
    "worldstream-studio",
    "worldstream-typescript-pack-sdk",
)

CLI_FIRST_PORTABLE_SUBJECT_ARTIFACT_IDS = tuple(
    artifact_id
    for artifact_id in LEGACY_PORTABLE_SUBJECT_ARTIFACT_IDS
    if artifact_id != "worldstream-studio"
)

SIDECAR_ARTIFACT_IDS = (
    "checksums",
    "sigstore-bundle",
    "spdx-sbom",
    "slsa-provenance",
)

LEGACY_NATIVE_BINARIES = ("worldstreamd", "worldstreamctl")
CLI_FIRST_NATIVE_BINARIES = (
    "worldstreamd",
    "worldstreamctl",
    # This is a retained headless compatibility name, not the retired web UI.
    "worldstream-studio-supervisor",
    "worldstream-assignment-mcp",
    "worldstream-managed-agent-host",
)


@dataclass(frozen=True)
class ReleaseInventory:
    identity: str
    manifest_schema: str
    subject_inventory_schema: str
    aggregation_schema: str
    build_type_version: int
    portable_subject_artifact_ids: tuple[str, ...]
    native_binaries: tuple[str, ...]
    serialized_discriminator: bool

    @property
    def payload_artifact_ids(self) -> tuple[str, ...]:
        return RUNTIME_PAYLOAD_ARTIFACT_IDS + self.portable_subject_artifact_ids

    @property
    def release_artifact_ids(self) -> tuple[str, ...]:
        return self.payload_artifact_ids + SIDECAR_ARTIFACT_IDS


LEGACY = ReleaseInventory(
    identity=LEGACY_RELEASE_INVENTORY,
    manifest_schema="worldstream/release-artifact-manifest/v2",
    subject_inventory_schema="worldstream/release-subject-inventory/v1",
    aggregation_schema="worldstream/release-aggregation/v2",
    build_type_version=4,
    portable_subject_artifact_ids=LEGACY_PORTABLE_SUBJECT_ARTIFACT_IDS,
    native_binaries=LEGACY_NATIVE_BINARIES,
    serialized_discriminator=False,
)

CLI_FIRST = ReleaseInventory(
    identity=CLI_FIRST_RELEASE_INVENTORY,
    manifest_schema="worldstream/release-artifact-manifest/v3",
    subject_inventory_schema="worldstream/release-subject-inventory/v2",
    aggregation_schema="worldstream/release-aggregation/v3",
    build_type_version=5,
    portable_subject_artifact_ids=CLI_FIRST_PORTABLE_SUBJECT_ARTIFACT_IDS,
    native_binaries=CLI_FIRST_NATIVE_BINARIES,
    serialized_discriminator=True,
)

BY_ID = {profile.identity: profile for profile in (LEGACY, CLI_FIRST)}


def resolve(value: str | None) -> ReleaseInventory:
    """Resolve a caller selection while preserving the implicit legacy default."""

    if value is None or value == LEGACY.identity:
        return LEGACY
    try:
        return BY_ID[value]
    except KeyError as error:
        raise ValueError(f"unsupported release inventory: {value!r}") from error


def identity_from_manifest(value: object) -> ReleaseInventory:
    """Select and strictly bind a detached manifest schema to its inventory."""

    if not isinstance(value, dict):
        raise ValueError("release manifest must be an object")  # noqa: TRY004
    schema = value.get("schema")
    if schema == LEGACY.manifest_schema:
        if "release_inventory" in value:
            raise ValueError("legacy release manifest must not add an inventory field")
        return LEGACY
    if schema == CLI_FIRST.manifest_schema:
        inventory = value.get("release_inventory")
        if inventory != CLI_FIRST.identity:
            raise ValueError(
                "CLI-first release manifest has a missing or mismatched inventory"
            )
        return CLI_FIRST
    raise ValueError(f"unsupported release manifest schema: {schema!r}")


def identity_from_subject_inventory(value: object) -> ReleaseInventory:
    """Select and strictly bind a pre-sign inventory schema to its profile."""

    if not isinstance(value, dict):
        raise ValueError("subject inventory must be an object")  # noqa: TRY004
    schema = value.get("schema")
    if schema == LEGACY.subject_inventory_schema:
        if "release_inventory" in value:
            raise ValueError("legacy subject inventory must not add an inventory field")
        return LEGACY
    if schema == CLI_FIRST.subject_inventory_schema:
        inventory = value.get("release_inventory")
        if inventory != CLI_FIRST.identity:
            raise ValueError(
                "CLI-first subject inventory has a missing or mismatched inventory"
            )
        return CLI_FIRST
    raise ValueError(f"unsupported subject inventory schema: {schema!r}")


def identity_from_aggregation(value: object) -> ReleaseInventory:
    """Select and strictly bind an aggregation schema to its release profile."""

    if not isinstance(value, dict):
        raise ValueError("release aggregation must be an object")  # noqa: TRY004
    schema = value.get("schema")
    if schema == LEGACY.aggregation_schema:
        if "release_inventory" in value:
            raise ValueError("legacy aggregation must not add an inventory field")
        return LEGACY
    if schema == CLI_FIRST.aggregation_schema:
        inventory = value.get("release_inventory")
        if inventory != CLI_FIRST.identity:
            raise ValueError(
                "CLI-first aggregation has a missing or mismatched inventory"
            )
        return CLI_FIRST
    raise ValueError(f"unsupported release aggregation schema: {schema!r}")

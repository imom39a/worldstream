//! Validated, registry-neutral Activity Client release and distribution contracts.

use std::collections::HashSet;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

pub const ACTIVITY_CLIENT_RELEASE_SCHEMA_V1: &str = "worldstream/activity-client-release/v1";
pub const ACTIVITY_DISTRIBUTION_SCHEMA_V1: &str = "worldstream/activity-distribution/v1";

const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 128;
const MAX_MEDIA_TYPE_BYTES: usize = 256;
const MAX_ENTRYPOINT_BYTES: usize = 1_024;
const MAX_ARTIFACTS: usize = 32;
const MAX_SURFACES: usize = 32;
const MAX_CAPABILITIES: usize = 32;
const MAX_CONFORMANCE_EVIDENCE: usize = 32;
const MAX_DISTRIBUTION_CLIENTS: usize = 64;
const MAX_DISTRIBUTION_PACKS: usize = 64;
const MAX_DISTRIBUTION_COMPATIBILITY: usize = 256;
const MAX_DISTRIBUTION_ARTIFACTS: usize = 128;
const MAX_ROLES: usize = 64;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityClientArtifactV1 {
    pub artifact_id: String,
    pub media_type: String,
    pub digest: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientSurfaceKindV1 {
    Browser,
    Cli,
    Tui,
    Native,
    Sdk,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityClientSurfaceV1 {
    pub surface_id: String,
    pub kind: ClientSurfaceKindV1,
    pub artifact_digest: String,
    pub entrypoint: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientConformanceEvidenceV1 {
    pub contract: String,
    pub evidence_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityClientReleaseV1 {
    pub schema: String,
    pub client_id: String,
    pub release_digest: String,
    pub client_contract: String,
    pub artifacts: Vec<ActivityClientArtifactV1>,
    pub surfaces: Vec<ActivityClientSurfaceV1>,
    pub conformance: Vec<ClientConformanceEvidenceV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactPackReferenceV1 {
    pub id: String,
    pub version: String,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityClientReleaseReferenceV1 {
    pub client_id: String,
    pub release_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityPackBundleReferenceV1 {
    pub pack: ExactPackReferenceV1,
    pub bundle_digest: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DistributionAccessModeV1 {
    Participant,
    Spectator,
    Operator,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityClientCompatibilityV1 {
    pub client_id: String,
    pub release_digest: String,
    pub surface_id: String,
    pub pack_revision_digest: String,
    pub access_mode: DistributionAccessModeV1,
    pub roles: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DistributionArtifactKindV1 {
    RunnerIntegration,
    Documentation,
    DeploymentTemplate,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DistributionArtifactReferenceV1 {
    pub artifact_id: String,
    pub kind: DistributionArtifactKindV1,
    pub media_type: String,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityDistributionV1 {
    pub schema: String,
    pub distribution_id: String,
    pub version: String,
    pub pack_bundles: Vec<ActivityPackBundleReferenceV1>,
    pub clients: Vec<ActivityClientReleaseReferenceV1>,
    pub client_compatibility: Vec<ActivityClientCompatibilityV1>,
    pub integration_artifacts: Vec<DistributionArtifactReferenceV1>,
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ActivityClientContractErrorV1 {
    #[error("activity client contract is invalid")]
    Invalid,
}

/// Decodes and validates one exact Activity Client Release document.
///
/// # Errors
///
/// Returns [`ActivityClientContractErrorV1::Invalid`] for an oversized,
/// malformed, unknown-field, or semantically invalid document.
pub fn read_activity_client_release(
    bytes: &[u8],
) -> Result<ActivityClientReleaseV1, ActivityClientContractErrorV1> {
    let release: ActivityClientReleaseV1 = read_document(bytes)?;
    validate_release(&release)?;
    Ok(release)
}

/// Decodes and validates one exact Activity Distribution document.
///
/// # Errors
///
/// Returns [`ActivityClientContractErrorV1::Invalid`] for an oversized,
/// malformed, unknown-field, or semantically invalid document.
pub fn read_activity_distribution(
    bytes: &[u8],
) -> Result<ActivityDistributionV1, ActivityClientContractErrorV1> {
    let distribution: ActivityDistributionV1 = read_document(bytes)?;
    validate_distribution(&distribution)?;
    Ok(distribution)
}

fn read_document<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ActivityClientContractErrorV1> {
    if bytes.is_empty() || bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(ActivityClientContractErrorV1::Invalid);
    }
    serde_json::from_slice(bytes).map_err(|_| ActivityClientContractErrorV1::Invalid)
}

fn validate_release(
    release: &ActivityClientReleaseV1,
) -> Result<(), ActivityClientContractErrorV1> {
    if release.schema != ACTIVITY_CLIENT_RELEASE_SCHEMA_V1
        || !is_identifier(&release.client_id)
        || !is_digest(&release.release_digest)
        || !is_contract(&release.client_contract)
        || release.artifacts.is_empty()
        || release.artifacts.len() > MAX_ARTIFACTS
        || release.surfaces.is_empty()
        || release.surfaces.len() > MAX_SURFACES
        || release.conformance.len() > MAX_CONFORMANCE_EVIDENCE
    {
        return Err(ActivityClientContractErrorV1::Invalid);
    }

    let mut artifact_ids = HashSet::with_capacity(release.artifacts.len());
    let mut artifact_digests = HashSet::with_capacity(release.artifacts.len());
    for artifact in &release.artifacts {
        if !is_identifier(&artifact.artifact_id)
            || !is_media_type(&artifact.media_type)
            || !is_digest(&artifact.digest)
            || !artifact_ids.insert(artifact.artifact_id.as_str())
        {
            return Err(ActivityClientContractErrorV1::Invalid);
        }
        artifact_digests.insert(artifact.digest.as_str());
    }

    let mut surface_ids = HashSet::with_capacity(release.surfaces.len());
    for surface in &release.surfaces {
        if !is_identifier(&surface.surface_id)
            || !artifact_digests.contains(surface.artifact_digest.as_str())
            || !is_entrypoint(&surface.entrypoint)
            || surface.capabilities.is_empty()
            || surface.capabilities.len() > MAX_CAPABILITIES
            || !surface_ids.insert(surface.surface_id.as_str())
            || !unique_valid_contract_values(&surface.capabilities)
        {
            return Err(ActivityClientContractErrorV1::Invalid);
        }
    }

    for evidence in &release.conformance {
        if !is_contract(&evidence.contract) || !is_digest(&evidence.evidence_digest) {
            return Err(ActivityClientContractErrorV1::Invalid);
        }
    }
    Ok(())
}

fn validate_distribution(
    distribution: &ActivityDistributionV1,
) -> Result<(), ActivityClientContractErrorV1> {
    if distribution.schema != ACTIVITY_DISTRIBUTION_SCHEMA_V1
        || !is_identifier(&distribution.distribution_id)
        || !is_version(&distribution.version)
        || distribution.pack_bundles.is_empty()
        || distribution.pack_bundles.len() > MAX_DISTRIBUTION_PACKS
        || distribution.clients.is_empty()
        || distribution.clients.len() > MAX_DISTRIBUTION_CLIENTS
        || distribution.client_compatibility.is_empty()
        || distribution.client_compatibility.len() > MAX_DISTRIBUTION_COMPATIBILITY
        || distribution.integration_artifacts.len() > MAX_DISTRIBUTION_ARTIFACTS
    {
        return Err(ActivityClientContractErrorV1::Invalid);
    }

    let mut pack_revisions = HashSet::with_capacity(distribution.pack_bundles.len());
    let mut pack_bundles = HashSet::with_capacity(distribution.pack_bundles.len());
    for reference in &distribution.pack_bundles {
        if !is_identifier(&reference.pack.id)
            || !is_version(&reference.pack.version)
            || !is_digest(&reference.pack.digest)
            || !is_digest(&reference.bundle_digest)
            || !pack_revisions.insert(reference.pack.digest.as_str())
            || !pack_bundles.insert(reference.bundle_digest.as_str())
        {
            return Err(ActivityClientContractErrorV1::Invalid);
        }
    }

    let mut clients = HashSet::with_capacity(distribution.clients.len());
    for client in &distribution.clients {
        if !is_identifier(&client.client_id)
            || !is_digest(&client.release_digest)
            || !clients.insert((client.client_id.as_str(), client.release_digest.as_str()))
        {
            return Err(ActivityClientContractErrorV1::Invalid);
        }
    }

    let mut compatibility = HashSet::with_capacity(distribution.client_compatibility.len());
    for claim in &distribution.client_compatibility {
        if !clients.contains(&(claim.client_id.as_str(), claim.release_digest.as_str()))
            || !is_identifier(&claim.surface_id)
            || !pack_revisions.contains(claim.pack_revision_digest.as_str())
            || claim.roles.len() > MAX_ROLES
            || !unique_valid_roles(&claim.roles)
            || !access_mode_roles_are_valid(claim.access_mode, &claim.roles)
            || !compatibility.insert((
                claim.client_id.as_str(),
                claim.release_digest.as_str(),
                claim.surface_id.as_str(),
                claim.pack_revision_digest.as_str(),
                claim.access_mode,
            ))
        {
            return Err(ActivityClientContractErrorV1::Invalid);
        }
    }

    let mut artifacts = HashSet::with_capacity(distribution.integration_artifacts.len());
    for artifact in &distribution.integration_artifacts {
        if !is_identifier(&artifact.artifact_id)
            || !is_media_type(&artifact.media_type)
            || !is_digest(&artifact.digest)
            || !artifacts.insert(artifact.artifact_id.as_str())
        {
            return Err(ActivityClientContractErrorV1::Invalid);
        }
    }
    Ok(())
}

fn unique_valid_contract_values(values: &[String]) -> bool {
    let mut unique = HashSet::with_capacity(values.len());
    values
        .iter()
        .all(|value| is_contract(value) && unique.insert(value.as_str()))
}

fn unique_valid_roles(values: &[String]) -> bool {
    let mut unique = HashSet::with_capacity(values.len());
    values.iter().all(|value| {
        !value.is_empty()
            && value.len() <= MAX_IDENTIFIER_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            && unique.insert(value.as_str())
    })
}

fn access_mode_roles_are_valid(access_mode: DistributionAccessModeV1, roles: &[String]) -> bool {
    match access_mode {
        DistributionAccessModeV1::Participant => !roles.is_empty(),
        DistributionAccessModeV1::Spectator | DistributionAccessModeV1::Operator => {
            roles.is_empty()
        }
    }
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn is_contract(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTIFIER_BYTES
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b'/')
        })
}

fn is_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_VERSION_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && byte != b'/' && byte != b'\\')
}

fn is_media_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_MEDIA_TYPE_BYTES
        && value.contains('/')
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn is_entrypoint(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ENTRYPOINT_BYTES
        && !value.contains(['?', '#', '\\', '\r', '\n'])
        && value.bytes().all(|byte| byte.is_ascii_graphic())
        && !value
            .split('/')
            .any(|segment| matches!(segment, "." | ".."))
}

fn is_digest(value: &str) -> bool {
    let payload = value
        .strip_prefix("blake3:")
        .or_else(|| value.strip_prefix("sha256:"));
    payload.is_some_and(|payload| {
        payload.len() == 64
            && payload
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const DIGEST_A: &str =
        "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DIGEST_B: &str =
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn release() -> serde_json::Value {
        json!({
            "schema": ACTIVITY_CLIENT_RELEASE_SCHEMA_V1,
            "client_id": "worldstream.agent-heist.web",
            "release_digest": DIGEST_A,
            "client_contract": "worldstream/activity-client-protocol/v1",
            "artifacts": [{
                "artifact_id": "browser-dist",
                "media_type": "application/vnd.worldstream.activity-client.web.v1+tar",
                "digest": DIGEST_B
            }],
            "surfaces": [{
                "surface_id": "participant-web",
                "kind": "browser",
                "artifact_digest": DIGEST_B,
                "entrypoint": "/agent-heist/",
                "capabilities": ["observe", "act", "replay"]
            }],
            "conformance": [{
                "contract": "worldstream/activity-client-protocol/v1",
                "evidence_digest": DIGEST_A
            }]
        })
    }

    #[test]
    fn accepts_an_immutable_registry_neutral_release() {
        let bytes = serde_json::to_vec(&release())
            .unwrap_or_else(|error| unreachable!("release fixture: {error}"));

        let parsed = read_activity_client_release(&bytes)
            .unwrap_or_else(|error| unreachable!("valid release: {error}"));

        assert_eq!(parsed.client_id, "worldstream.agent-heist.web");
        assert_eq!(parsed.release_digest, DIGEST_A);
        assert_eq!(parsed.surfaces[0].artifact_digest, DIGEST_B);
        assert_eq!(parsed.surfaces[0].entrypoint, "/agent-heist/");
    }

    #[test]
    fn rejects_runtime_and_authority_material() {
        for (field, value) in [
            ("launch_url", "http://127.0.0.1:5173/agent-heist/"),
            ("origin", "http://127.0.0.1:5173"),
            ("credential", "do-not-serialize"),
            ("secret", "do-not-serialize"),
        ] {
            let mut invalid = release();
            invalid
                .as_object_mut()
                .unwrap_or_else(|| unreachable!("release fixture is an object"))
                .insert(field.to_owned(), json!(value));
            let bytes = serde_json::to_vec(&invalid)
                .unwrap_or_else(|error| unreachable!("invalid fixture: {error}"));
            assert_eq!(
                read_activity_client_release(&bytes),
                Err(ActivityClientContractErrorV1::Invalid),
                "field {field} must not cross the release seam"
            );
        }
    }

    #[test]
    fn rejects_an_unsafe_or_missing_surface_entrypoint() {
        for entrypoint in ["", "/agent-heist/../studio/", "/agent-heist/?token=secret"] {
            let mut invalid = release();
            invalid["surfaces"][0]["entrypoint"] = json!(entrypoint);
            let bytes = serde_json::to_vec(&invalid)
                .unwrap_or_else(|error| unreachable!("invalid fixture: {error}"));
            assert_eq!(
                read_activity_client_release(&bytes),
                Err(ActivityClientContractErrorV1::Invalid),
            );
        }
    }

    #[test]
    fn retains_exact_pack_and_independent_client_release_references() {
        let bytes = serde_json::to_vec(&json!({
            "schema": ACTIVITY_DISTRIBUTION_SCHEMA_V1,
            "distribution_id": "worldstream.agent-heist.local-demo",
            "version": "1.0.0",
            "pack_bundles": [{
                "pack": {
                    "id": "worldstream.agent-heist",
                    "version": "0.2.0",
                    "digest": DIGEST_A
                },
                "bundle_digest": DIGEST_B
            }],
            "clients": [
                { "client_id": "worldstream.agent-heist.web", "release_digest": DIGEST_B },
                { "client_id": "example.agent-heist.tui", "release_digest": DIGEST_A }
            ],
            "client_compatibility": [{
                "client_id": "worldstream.agent-heist.web",
                "release_digest": DIGEST_B,
                "surface_id": "participant-web",
                "pack_revision_digest": DIGEST_A,
                "access_mode": "participant",
                "roles": ["navigator", "insider", "broker"]
            }],
            "integration_artifacts": [{
                "artifact_id": "deployment-example",
                "kind": "deployment_template",
                "media_type": "application/json",
                "digest": DIGEST_A
            }]
        }))
        .unwrap_or_else(|error| unreachable!("distribution fixture: {error}"));

        let parsed = read_activity_distribution(&bytes)
            .unwrap_or_else(|error| unreachable!("valid distribution: {error}"));

        assert_eq!(parsed.pack_bundles[0].pack.digest, DIGEST_A);
        assert_eq!(parsed.pack_bundles[0].bundle_digest, DIGEST_B);
        assert_eq!(parsed.clients.len(), 2);
        assert_eq!(parsed.client_compatibility.len(), 1);
        assert_ne!(parsed.clients[0].client_id, parsed.clients[1].client_id);
    }

    #[test]
    fn distribution_rejects_semantic_only_packs_and_invalid_role_claims() {
        let semantic_only = serde_json::to_vec(&json!({
            "schema": ACTIVITY_DISTRIBUTION_SCHEMA_V1,
            "distribution_id": "example.incomplete",
            "version": "1",
            "pack_bundles": [{
                "pack": { "id": "example.pack", "version": "1", "digest": DIGEST_A }
            }],
            "clients": [{ "client_id": "example.client", "release_digest": DIGEST_B }],
            "client_compatibility": [{
                "client_id": "example.client",
                "release_digest": DIGEST_B,
                "surface_id": "browser",
                "pack_revision_digest": DIGEST_A,
                "access_mode": "spectator",
                "roles": ["participant-role"]
            }],
            "integration_artifacts": []
        }))
        .unwrap_or_else(|error| unreachable!("distribution fixture: {error}"));

        assert_eq!(
            read_activity_distribution(&semantic_only),
            Err(ActivityClientContractErrorV1::Invalid),
        );
    }
}

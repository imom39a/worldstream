use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The compatibility artifact compiled into every runtime-linked binary.
pub const EMBEDDED_COMPATIBILITY_JSON: &str = include_str!("../../../compatibility.json");

const MANIFEST_SCHEMA_V1: &str = "worldstream/storage-compatibility-manifest/v1";
const COUNTER_PACK_ID: &str = "worldstream.counter";
const AGENT_HEIST_PACK_ID: &str = "worldstream.agent-heist";
const NEGOTIATE_PACK_ID: &str = "worldstream.negotiate";

/// Compatibility fields consumed by the process shell.
#[derive(Clone, Debug, Deserialize)]
pub struct CompatibilityManifest {
    /// Manifest schema identifier.
    pub schema: String,
    /// Monotonic manifest schema revision.
    pub manifest_revision: u32,
    /// Specification or release artifact kind.
    pub manifest_kind: String,
    /// Whether the embedded compatibility contract is complete and buildable.
    /// Final artifact/evidence verification is detached per ADR 0012.
    pub release_ready: bool,
    /// Release candidate version.
    pub release_candidate: String,
    /// Frozen product contracts.
    pub contracts: CompatibilityContracts,
    /// Closed Genesis-selected canonical record tuples supported by this build.
    pub canonical_lineages: Vec<CanonicalLineageSummary>,
    /// Pinned compiler toolchain.
    pub toolchains: CompatibilityToolchains,
    /// Supported storage selector values.
    pub storage: CompatibilityStorage,
    /// Exact compiled pack revisions retained by this build.
    pub pack_executors: Vec<PackExecutorManifestEntry>,
}

/// One exact Activity Pack revision compiled into the release candidate.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PackExecutorManifestEntry {
    pack_id: String,
    explanatory_version: String,
    host_contract_id: String,
    revision_lock_id: String,
    revision_digest_algorithm: String,
    revision_digest: String,
    descriptor_digest: String,
    executor_artifact_digest: String,
    schema_bundle_digest: String,
    codec_bundle_digest: String,
    golden_corpus_digest: String,
    selectable_for_new_rooms: bool,
    runnable_for_retained_rooms: bool,
    status: String,
    required_for_release: bool,
}

/// Bounded operator-facing identity and retention status for one pack revision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PackExecutorSummary {
    /// Stable logical pack identity.
    pub pack_id: String,
    /// Human-facing version label; the digest selects behavior.
    pub explanatory_version: String,
    /// Exact semantic revision digest, or empty only for an unresolved specification entry.
    pub revision_digest: String,
    /// Whether this revision may create new Rooms.
    pub selectable_for_new_rooms: bool,
    /// Whether this revision may execute retained Room lineage.
    pub runnable_for_retained_rooms: bool,
    /// `unresolved` for a specification placeholder, otherwise `resolved`.
    pub status: String,
    /// Whether release evidence must cover this entry.
    pub required_for_release: bool,
}

/// Contract versions reported by `/version`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CompatibilityContracts {
    /// Product version.
    pub product: String,
    /// Wire protocol version.
    pub wire: String,
    /// Configuration schema version.
    pub config: u32,
    /// Storage schema version.
    pub storage_schema: u32,
    /// Canonical Core schema identifier.
    pub core_schema_version: String,
    /// Canonical hash suite identifier.
    pub hash_suite: String,
}

/// Exact immutable tuple for one supported canonical Room history format.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalLineageSummary {
    /// Immutable format selected by Genesis.
    pub canonical_history_format: String,
    /// Exact Genesis record version.
    pub genesis_version: String,
    /// Exact Genesis codec identity.
    pub genesis_codec_id: String,
    /// Exact Genesis hash framing identity.
    pub genesis_hash_suite: String,
    /// Exact Transition record version.
    pub transition_version: String,
    /// Exact Transition codec identity.
    pub transition_codec_id: String,
    /// Exact Transition hash framing identity.
    pub transition_hash_suite: String,
    /// Frozen admission policy, present only for compact lineages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_budget_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompatibilityToolchains {
    rust: String,
    rust_edition: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CompatibilityStorage {
    profiles: Vec<String>,
    default_profile: String,
    sqlite: CompatibilitySqlite,
    postgresql: CompatibilityPostgresql,
}

#[derive(Clone, Debug, Deserialize)]
struct CompatibilitySqlite {
    profile_id: String,
    version: String,
}

#[derive(Clone, Debug, Deserialize)]
struct CompatibilityPostgresql {
    profile_id: String,
    major: u32,
    minimum_patch: String,
}

/// Stable manifest details safe to expose through operator surfaces.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompatibilitySummary {
    /// Manifest schema identifier.
    pub schema: String,
    /// Manifest revision.
    pub revision: u32,
    /// Specification/release artifact kind.
    pub kind: String,
    /// Whether the embedded contract is complete; detached release evidence is
    /// verified separately and is not inferred by the running process.
    pub release_ready: bool,
    /// Canonical product and protocol contracts.
    pub contracts: CompatibilityContracts,
    /// Exact supported lineage tuples. State hash domains keep their V1 identity.
    pub canonical_lineages: Vec<CanonicalLineageSummary>,
    /// Exact Rust toolchain pin.
    pub rust_toolchain: String,
    /// Exact Rust edition.
    pub rust_edition: String,
    /// Exact `SQLite` build version named by the authored specification.
    pub sqlite_version: String,
    /// Supported `PostgreSQL` major.
    pub postgresql_major: u32,
    /// Minimum supported `PostgreSQL` patch.
    pub postgresql_minimum_patch: String,
    /// Every embedded Activity Pack revision and its retention status.
    pub pack_executors: Vec<PackExecutorSummary>,
}

impl CompatibilityManifest {
    /// Validates the subset required before the process shell can start.
    ///
    /// # Errors
    ///
    /// Returns an error when the embedded artifact declares a schema, revision,
    /// config/toolchain pin, or storage selector the compiled shell cannot honor.
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.schema != MANIFEST_SCHEMA_V1 {
            return Err(ManifestError::UnsupportedSchema(self.schema.clone()));
        }
        if self.manifest_revision != 1 {
            return Err(ManifestError::UnsupportedRevision(self.manifest_revision));
        }
        if !matches!(self.manifest_kind.as_str(), "specification" | "release")
            || (self.manifest_kind == "specification") == self.release_ready
        {
            return Err(ManifestError::Inconsistent(
                "specification manifests must be release_ready=false and release manifests true",
            ));
        }
        if self.release_candidate != self.contracts.product {
            return Err(ManifestError::Inconsistent(
                "release_candidate must equal contracts.product",
            ));
        }
        self.validate_canonical_lineages()?;
        if self.contracts.config != 1 {
            return Err(ManifestError::UnsupportedConfig(self.contracts.config));
        }
        if self.toolchains.rust != "1.97.1" || self.toolchains.rust_edition != "2024" {
            return Err(ManifestError::Inconsistent(
                "embedded Rust toolchain does not match the workspace pin",
            ));
        }
        if self.storage.profiles.as_slice() != ["sqlite-bundled", "postgres-primary"] {
            return Err(ManifestError::Inconsistent(
                "storage.profiles must contain only the two frozen profiles in order",
            ));
        }
        if self.storage.default_profile != "sqlite-bundled"
            || self.storage.sqlite.profile_id != "sqlite-bundled"
            || self.storage.postgresql.profile_id != "postgres-primary"
        {
            return Err(ManifestError::Inconsistent(
                "storage profile identifiers do not match the frozen selectors",
            ));
        }
        self.validate_pack_executors()?;
        Ok(())
    }

    fn validate_canonical_lineages(&self) -> Result<(), ManifestError> {
        let rows = &self.canonical_lineages;
        if self.contracts.hash_suite != "blake3-canonical-json-v1"
            || rows.len() != 2
            || rows[0]
                != (CanonicalLineageSummary {
                    canonical_history_format: "worldstream/transition/v1".to_owned(),
                    genesis_version: "worldstream/genesis/v1".to_owned(),
                    genesis_codec_id: "worldstream/canonical-json/v1".to_owned(),
                    genesis_hash_suite: "blake3-canonical-json-v1".to_owned(),
                    transition_version: "worldstream/transition/v1".to_owned(),
                    transition_codec_id: "worldstream/canonical-json/v1".to_owned(),
                    transition_hash_suite: "blake3-canonical-json-v1".to_owned(),
                    payload_budget_id: None,
                })
            || rows[1]
                != (CanonicalLineageSummary {
                    canonical_history_format: "worldstream/transition/v2".to_owned(),
                    genesis_version: "worldstream/genesis/v2".to_owned(),
                    genesis_codec_id: "worldstream/genesis-record/v2".to_owned(),
                    genesis_hash_suite: "blake3-canonical-json-v2".to_owned(),
                    transition_version: "worldstream/transition/v2".to_owned(),
                    transition_codec_id: "worldstream/transition-record/v2".to_owned(),
                    transition_hash_suite: "blake3-canonical-json-v2".to_owned(),
                    payload_budget_id: Some("worldstream/payload-budget/v1".to_owned()),
                })
        {
            return Err(ManifestError::Inconsistent(
                "unsupported canonical lineage inventory",
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn validate_pack_executors(&self) -> Result<(), ManifestError> {
        if self.pack_executors.is_empty() {
            return Err(ManifestError::Inconsistent(
                "pack executor list must not be empty",
            ));
        }
        let mut revision_digests = BTreeSet::new();
        let mut required_pack_ids = BTreeSet::new();
        let mut counter_versions = BTreeSet::new();
        let mut selectable_public_activity = false;
        for entry in &self.pack_executors {
            if entry.pack_id.is_empty() || entry.explanatory_version.is_empty() {
                return Err(ManifestError::Inconsistent(
                    "pack executor identity fields must be nonempty",
                ));
            }
            if entry.host_contract_id != "worldstream/activity-pack/v1"
                || entry.revision_lock_id != "worldstream/pack-revision-lock/v1"
                || entry.revision_digest_algorithm != "blake3"
            {
                return Err(ManifestError::Inconsistent(
                    "pack executor declares an unsupported contract, revision lock, or digest algorithm",
                ));
            }
            if entry.selectable_for_new_rooms && !entry.runnable_for_retained_rooms {
                return Err(ManifestError::Inconsistent(
                    "selectable pack executor must be runnable for retained Rooms",
                ));
            }
            if entry.pack_id == COUNTER_PACK_ID {
                if !matches!(
                    entry.explanatory_version.as_str(),
                    "1.0.0" | "2.0.0" | "3.0.0" | "4.0.0"
                ) || !counter_versions.insert(entry.explanatory_version.as_str())
                {
                    return Err(ManifestError::Inconsistent(
                        "Counter executor set must contain each frozen revision exactly once",
                    ));
                }
                if entry.selectable_for_new_rooms
                    || !entry.runnable_for_retained_rooms
                    || !entry.required_for_release
                    || entry.status != "resolved"
                {
                    return Err(ManifestError::Inconsistent(
                        "Counter revisions must be resolved, required, retained-runnable, and nonselectable",
                    ));
                }
            }

            let digests = [
                &entry.revision_digest,
                &entry.descriptor_digest,
                &entry.executor_artifact_digest,
                &entry.schema_bundle_digest,
                &entry.codec_bundle_digest,
                &entry.golden_corpus_digest,
            ];
            match entry.status.as_str() {
                "unresolved" => {
                    if self.release_ready
                        || entry.selectable_for_new_rooms
                        || entry.runnable_for_retained_rooms
                        || !entry.required_for_release
                        || digests.iter().any(|digest| !digest.is_empty())
                    {
                        return Err(ManifestError::Inconsistent(
                            "unresolved required pack executor is allowed only in a non-release manifest with empty digests and disabled selection/execution",
                        ));
                    }
                }
                "resolved" => {
                    if digests
                        .iter()
                        .any(|digest| !is_canonical_blake3_digest(digest))
                    {
                        return Err(ManifestError::Inconsistent(
                            "resolved pack executor digest is missing or noncanonical",
                        ));
                    }
                    if !revision_digests.insert(&entry.revision_digest) {
                        return Err(ManifestError::Inconsistent(
                            "pack executor revision digest collision",
                        ));
                    }
                    if entry.required_for_release && !entry.runnable_for_retained_rooms {
                        return Err(ManifestError::Inconsistent(
                            "required resolved pack executor must be runnable for retained Rooms",
                        ));
                    }
                    if entry.required_for_release
                        && entry.selectable_for_new_rooms
                        && entry.runnable_for_retained_rooms
                        && entry.pack_id == NEGOTIATE_PACK_ID
                    {
                        selectable_public_activity = true;
                    }
                }
                _ => {
                    return Err(ManifestError::Inconsistent(
                        "pack executor status must be unresolved or resolved",
                    ));
                }
            }
            if entry.required_for_release {
                required_pack_ids.insert(entry.pack_id.as_str());
            }
        }
        if !required_pack_ids.contains(COUNTER_PACK_ID)
            || !required_pack_ids.contains(AGENT_HEIST_PACK_ID)
            || !required_pack_ids.contains(NEGOTIATE_PACK_ID)
        {
            return Err(ManifestError::Inconsistent(
                "required pack executor entries must include Counter, Agent Heist, and Negotiate",
            ));
        }
        if counter_versions != BTreeSet::from(["1.0.0", "2.0.0", "3.0.0", "4.0.0"]) {
            return Err(ManifestError::Inconsistent(
                "manifest must retain exact Counter v1, v2, v3, and v4 executors",
            ));
        }
        if self.release_ready && !selectable_public_activity {
            return Err(ManifestError::Inconsistent(
                "release-ready manifest requires a selectable runnable Negotiate Activity Pack",
            ));
        }
        Ok(())
    }

    /// Returns whether a startup storage selector is declared by the manifest.
    #[must_use]
    pub fn supports_storage_profile(&self, profile: &str) -> bool {
        self.storage.profiles.iter().any(|item| item == profile)
    }

    /// Produces the bounded operator-facing manifest view.
    #[must_use]
    pub fn summary(&self) -> CompatibilitySummary {
        CompatibilitySummary {
            schema: self.schema.clone(),
            revision: self.manifest_revision,
            kind: self.manifest_kind.clone(),
            release_ready: self.release_ready,
            contracts: self.contracts.clone(),
            canonical_lineages: self.canonical_lineages.clone(),
            rust_toolchain: self.toolchains.rust.clone(),
            rust_edition: self.toolchains.rust_edition.clone(),
            sqlite_version: self.storage.sqlite.version.clone(),
            postgresql_major: self.storage.postgresql.major,
            postgresql_minimum_patch: self.storage.postgresql.minimum_patch.clone(),
            pack_executors: self
                .pack_executors
                .iter()
                .map(|entry| PackExecutorSummary {
                    pack_id: entry.pack_id.clone(),
                    explanatory_version: entry.explanatory_version.clone(),
                    revision_digest: entry.revision_digest.clone(),
                    selectable_for_new_rooms: entry.selectable_for_new_rooms,
                    runnable_for_retained_rooms: entry.runnable_for_retained_rooms,
                    status: entry.status.clone(),
                    required_for_release: entry.required_for_release,
                })
                .collect(),
        }
    }
}

fn is_canonical_blake3_digest(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("blake3:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Returns the exact embedded canonical JSON bytes as UTF-8 text.
#[must_use]
pub const fn embedded_manifest_json() -> &'static str {
    EMBEDDED_COMPATIBILITY_JSON
}

/// Parses and validates the build-guarded embedded compatibility artifact.
///
/// # Errors
///
/// Returns an error when the JSON is malformed or its frozen identity is invalid.
pub fn embedded_manifest() -> Result<CompatibilityManifest, ManifestError> {
    let manifest: CompatibilityManifest = serde_json::from_str(EMBEDDED_COMPATIBILITY_JSON)?;
    manifest.validate()?;
    Ok(manifest)
}

/// Fail-closed embedded manifest validation errors.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The embedded JSON is malformed or has a wrong field type.
    #[error("embedded compatibility JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// The schema is not implemented by this process.
    #[error("unsupported compatibility schema: {0}")]
    UnsupportedSchema(String),
    /// The manifest revision is not implemented by this process.
    #[error("unsupported compatibility manifest revision: {0}")]
    UnsupportedRevision(u32),
    /// The configuration contract is not implemented by this process.
    #[error("unsupported configuration contract version: {0}")]
    UnsupportedConfig(u32),
    /// Related manifest values contradict one another or the compiled contract.
    #[error("inconsistent compatibility manifest: {0}")]
    Inconsistent(&'static str),
}

#[cfg(test)]
mod tests {
    use super::{CompatibilityManifest, embedded_manifest, embedded_manifest_json};

    fn manifest() -> CompatibilityManifest {
        embedded_manifest()
            .unwrap_or_else(|error| unreachable!("embedded manifest is valid: {error}"))
    }

    fn resolve(entry: &mut super::PackExecutorManifestEntry, hex: char) {
        entry.status = "resolved".to_owned();
        let digest = format!("blake3:{}", hex.to_string().repeat(64));
        for digest_field in [
            &mut entry.revision_digest,
            &mut entry.descriptor_digest,
            &mut entry.executor_artifact_digest,
            &mut entry.schema_bundle_digest,
            &mut entry.codec_bundle_digest,
            &mut entry.golden_corpus_digest,
        ] {
            *digest_field = digest.clone();
        }
    }

    fn entry_mut<'a>(
        manifest: &'a mut CompatibilityManifest,
        pack_id: &str,
        version: &str,
    ) -> &'a mut super::PackExecutorManifestEntry {
        manifest
            .pack_executors
            .iter_mut()
            .find(|entry| entry.pack_id == pack_id && entry.explanatory_version == version)
            .unwrap_or_else(|| unreachable!("authored manifest entry {pack_id} {version}"))
    }

    fn release_manifest() -> CompatibilityManifest {
        let mut manifest = manifest();
        manifest.manifest_kind = "release".to_owned();
        manifest.release_ready = true;
        let negotiate = entry_mut(&mut manifest, super::NEGOTIATE_PACK_ID, "0.1.0");
        resolve(negotiate, 'd');
        negotiate.selectable_for_new_rooms = true;
        negotiate.runnable_for_retained_rooms = true;
        manifest
    }

    #[test]
    fn embedded_manifest_is_valid_and_fail_closed() {
        let manifest = embedded_manifest();
        assert!(manifest.is_ok());
        let manifest = manifest.unwrap_or_else(|error| unreachable!("validated above: {error}"));
        assert_eq!(manifest.manifest_kind, "specification");
        assert!(!manifest.release_ready);
        assert_eq!(manifest.contracts.config, 1);
    }

    #[test]
    fn embedded_artifact_ends_with_a_single_newline() {
        let bytes = embedded_manifest_json().as_bytes();
        assert!(bytes.ends_with(b"\n"));
        assert!(!bytes.ends_with(b"\n\n"));
    }

    #[test]
    fn lineage_inventory_rejects_unknown_or_inconsistent_selection() {
        let valid = manifest();
        assert_eq!(valid.canonical_lineages.len(), 2);
        assert_eq!(valid.contracts.hash_suite, "blake3-canonical-json-v1");
        assert!(valid.canonical_lineages[0].payload_budget_id.is_none());

        let mut reordered = manifest();
        reordered.canonical_lineages.swap(0, 1);
        assert!(reordered.validate().is_err());

        let mut unknown_codec = manifest();
        unknown_codec.canonical_lineages[1].transition_codec_id = "unknown".to_owned();
        assert!(unknown_codec.validate().is_err());

        let mut missing_policy = manifest();
        missing_policy.canonical_lineages[1].payload_budget_id = None;
        assert!(missing_policy.validate().is_err());

        let mut changed_legacy_suite = manifest();
        changed_legacy_suite.contracts.hash_suite = "blake3-canonical-json-v2".to_owned();
        assert!(changed_legacy_suite.validate().is_err());
    }

    #[test]
    fn retained_demo_entries_and_official_negotiate_are_resolved() {
        let summary = manifest().summary();
        assert_eq!(summary.pack_executors.len(), 8);
        let counter: Vec<_> = summary
            .pack_executors
            .iter()
            .filter(|entry| entry.pack_id == super::COUNTER_PACK_ID)
            .collect();
        assert_eq!(counter.len(), 4);
        assert!(counter.iter().all(|entry| {
            entry.status == "resolved"
                && !entry.revision_digest.is_empty()
                && !entry.selectable_for_new_rooms
                && entry.runnable_for_retained_rooms
        }));
        let heists: Vec<_> = summary
            .pack_executors
            .iter()
            .filter(|entry| entry.pack_id == super::AGENT_HEIST_PACK_ID)
            .collect();
        assert_eq!(heists.len(), 3);
        assert!(heists.iter().all(|entry| {
            entry.status == "resolved"
                && !entry.revision_digest.is_empty()
                && entry.runnable_for_retained_rooms
        }));
        assert!(heists.iter().any(|entry| {
            entry.explanatory_version == "0.1.0" && entry.selectable_for_new_rooms
        }));
        assert!(heists.iter().any(|entry| {
            entry.explanatory_version == "0.2.0" && entry.selectable_for_new_rooms
        }));
        assert!(heists.iter().any(|entry| {
            entry.explanatory_version == "0.0.1" && !entry.selectable_for_new_rooms
        }));
        let negotiate = summary
            .pack_executors
            .iter()
            .find(|entry| entry.pack_id == super::NEGOTIATE_PACK_ID)
            .unwrap_or_else(|| unreachable!("Negotiate specification row"));
        assert_eq!(negotiate.status, "resolved");
        assert_eq!(
            negotiate.revision_digest,
            "blake3:a62585c88ffebe0b2222f5f93e17de1e9cbb003593eca4891225f75dca985589"
        );
        assert!(negotiate.selectable_for_new_rooms);
        assert!(negotiate.runnable_for_retained_rooms);
        assert!(negotiate.required_for_release);
    }

    #[test]
    fn pack_executor_validation_rejects_selectable_nonrunnable_and_invalid_status() {
        let mut invalid = manifest();
        invalid.pack_executors[0].selectable_for_new_rooms = true;
        invalid.pack_executors[0].runnable_for_retained_rooms = false;
        assert!(invalid.validate().is_err());

        let mut invalid = manifest();
        invalid.pack_executors[0].status = "pending".to_owned();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn resolved_pack_executors_require_canonical_unique_digests() {
        let mut resolved = manifest();
        resolve(&mut resolved.pack_executors[0], 'a');
        assert!(resolved.validate().is_ok());

        let mut collision = resolved.clone();
        let duplicate = collision.pack_executors[0].clone();
        collision.pack_executors.push(duplicate);
        assert!(collision.validate().is_err());

        resolved.pack_executors[0].descriptor_digest = "not-a-digest".to_owned();
        assert!(resolved.validate().is_err());
    }

    #[test]
    fn unresolved_entries_reject_release_manifests_or_partial_digests() {
        let mut release = release_manifest();
        let unresolved = entry_mut(&mut release, super::AGENT_HEIST_PACK_ID, "0.2.0");
        unresolved.status = "unresolved".to_owned();
        for digest in [
            &mut unresolved.revision_digest,
            &mut unresolved.descriptor_digest,
            &mut unresolved.executor_artifact_digest,
            &mut unresolved.schema_bundle_digest,
            &mut unresolved.codec_bundle_digest,
            &mut unresolved.golden_corpus_digest,
        ] {
            digest.clear();
        }
        assert!(release.validate().is_err());

        let mut partial = manifest();
        let partial_entry = entry_mut(&mut partial, super::NEGOTIATE_PACK_ID, "0.1.0");
        partial_entry.status = "unresolved".to_owned();
        partial_entry.selectable_for_new_rooms = false;
        partial_entry.runnable_for_retained_rooms = false;
        for digest in [
            &mut partial_entry.revision_digest,
            &mut partial_entry.descriptor_digest,
            &mut partial_entry.executor_artifact_digest,
            &mut partial_entry.schema_bundle_digest,
            &mut partial_entry.codec_bundle_digest,
            &mut partial_entry.golden_corpus_digest,
        ] {
            digest.clear();
        }
        partial_entry.revision_digest =
            "blake3:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned();
        assert!(partial.validate().is_err());
    }

    #[test]
    fn pack_executor_validation_requires_the_frozen_required_pack_entries() {
        let mut empty = manifest();
        empty.pack_executors.clear();
        assert!(empty.validate().is_err());

        let mut missing_counter = manifest();
        missing_counter
            .pack_executors
            .retain(|entry| entry.pack_id != super::COUNTER_PACK_ID);
        assert!(missing_counter.validate().is_err());

        let mut missing_heist = manifest();
        missing_heist
            .pack_executors
            .retain(|entry| entry.pack_id != super::AGENT_HEIST_PACK_ID);
        assert!(missing_heist.validate().is_err());

        let mut missing_negotiate = manifest();
        missing_negotiate
            .pack_executors
            .retain(|entry| entry.pack_id != super::NEGOTIATE_PACK_ID);
        assert!(missing_negotiate.validate().is_err());
    }

    #[test]
    fn required_resolved_pack_executor_must_be_runnable() {
        let mut invalid = manifest();
        resolve(&mut invalid.pack_executors[0], 'a');
        invalid.pack_executors[0].runnable_for_retained_rooms = false;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn counter_revisions_are_never_selectable_for_new_rooms() {
        let mut invalid = manifest();
        entry_mut(&mut invalid, super::COUNTER_PACK_ID, "1.0.0").selectable_for_new_rooms = true;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn manifest_requires_exact_resolved_counter_v1_v2_v3_and_v4_rows() {
        let mut missing_v2 = manifest();
        missing_v2.pack_executors.retain(|entry| {
            entry.pack_id != super::COUNTER_PACK_ID || entry.explanatory_version != "2.0.0"
        });
        assert!(missing_v2.validate().is_err());

        let mut missing_v3 = manifest();
        missing_v3.pack_executors.retain(|entry| {
            entry.pack_id != super::COUNTER_PACK_ID || entry.explanatory_version != "3.0.0"
        });
        assert!(missing_v3.validate().is_err());

        let mut missing_v4 = manifest();
        missing_v4.pack_executors.retain(|entry| {
            entry.pack_id != super::COUNTER_PACK_ID || entry.explanatory_version != "4.0.0"
        });
        assert!(missing_v4.validate().is_err());

        let mut wrong_version = manifest();
        entry_mut(&mut wrong_version, super::COUNTER_PACK_ID, "2.0.0").explanatory_version =
            "3.0.0".to_owned();
        assert!(wrong_version.validate().is_err());

        let mut unresolved = manifest();
        let counter = entry_mut(&mut unresolved, super::COUNTER_PACK_ID, "1.0.0");
        counter.status = "unresolved".to_owned();
        for digest in [
            &mut counter.revision_digest,
            &mut counter.descriptor_digest,
            &mut counter.executor_artifact_digest,
            &mut counter.schema_bundle_digest,
            &mut counter.codec_bundle_digest,
            &mut counter.golden_corpus_digest,
        ] {
            digest.clear();
        }
        assert!(unresolved.validate().is_err());
    }

    #[test]
    fn release_ready_manifest_rejects_no_selectable_public_activity() {
        let mut no_selectable_public_activity = release_manifest();
        let negotiate = entry_mut(
            &mut no_selectable_public_activity,
            super::NEGOTIATE_PACK_ID,
            "0.1.0",
        );
        negotiate.selectable_for_new_rooms = false;
        assert!(no_selectable_public_activity.validate().is_err());
    }

    #[test]
    fn release_ready_manifest_requires_selectable_negotiate_not_an_arbitrary_pack() {
        let mut invalid = release_manifest();
        entry_mut(&mut invalid, super::NEGOTIATE_PACK_ID, "0.1.0").selectable_for_new_rooms = false;

        let mut substitute = invalid
            .pack_executors
            .iter()
            .find(|entry| entry.pack_id == super::COUNTER_PACK_ID)
            .unwrap_or_else(|| unreachable!("Counter row"))
            .clone();
        substitute.pack_id = "worldstream.other".to_owned();
        substitute.explanatory_version = "1.0.0".to_owned();
        substitute.selectable_for_new_rooms = true;
        resolve(&mut substitute, 'c');
        invalid.pack_executors.push(substitute);

        assert!(invalid.validate().is_err());
    }
}

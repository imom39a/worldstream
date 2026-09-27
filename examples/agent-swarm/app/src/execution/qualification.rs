//! Exact native provider-qualification records.
//!
//! Static `--help` inspection can establish that a flag exists, but it cannot
//! establish subscription login, effective-setting reports, delegation
//! containment, or native descendant cancellation. This module applies those
//! stronger claims only after re-hashing a content-addressed native evidence
//! bundle, deriving its controls from typed receipts, and binding the result to
//! the exact executable digest, version, platform, and model/effort pair.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write as _,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;
use worldstream_runtime::{
    create_owner_only_file, prepare_data_directory, validate_owner_only_file,
};

use super::{
    NativeProviderProbe, ProviderBlocker, ProviderCapabilities, ProviderInspection, ProviderKind,
    ProviderQualificationBinding, ProviderRegistry, QualifiedSelection,
};

const QUALIFICATION_SCHEMA: &str = "worldstream/agent-swarm-provider-qualification/v2";
const INSTALLED_QUALIFICATION_SCHEMA: &str =
    "worldstream/agent-swarm-installed-provider-qualification/v2";
const EVIDENCE_SCHEMA: &str = "worldstream/agent-swarm-qualification-evidence/v2";
const INVOCATION_RECEIPT_SCHEMA: &str = "worldstream/agent-swarm-provider-invocation-receipt/v1";
const LOGIN_RECEIPT_SCHEMA: &str = "worldstream/agent-swarm-provider-login-receipt/v1";
const PROCESS_RECEIPT_SCHEMA: &str = "worldstream/agent-swarm-process-containment/v1";
const RESOURCE_CONFINEMENT_RECEIPT_SCHEMA: &str = "worldstream/agent-swarm-resource-confinement/v1";
const INSTALL_RECEIPT_SCHEMA: &str = "worldstream/agent-swarm-install/v1";
const SCHEDULER_RECEIPT_SCHEMA: &str = "worldstream/agent-swarm-scheduler-trace/v1";
const SCENARIO_RECEIPT_SCHEMA: &str = "worldstream/agent-swarm-scenario-receipt/v1";
const ACTION_PROPOSAL_SCHEMA: &str = "worldstream/agent-swarm-action-proposal@1";
const COORDINATION_ENGINE: &str = "worldstream-agent-swarm";
const MAX_QUALIFICATION_BYTES: u64 = 1024 * 1024;
const MAX_EVIDENCE_RECEIPTS: usize = 512;
const MAX_SELECTIONS: usize = 128;
const MAX_TEXT_BYTES: usize = 4096;
const DIRECT_API_ENVIRONMENT: [&str; 21] = [
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_FOUNDRY_API_KEY",
    "ANTHROPIC_FOUNDRY_BASE_URL",
    "ANTHROPIC_FOUNDRY_RESOURCE",
    "ANTHROPIC_VERTEX_BASE_URL",
    "AWS_ACCESS_KEY_ID",
    "AWS_BEARER_TOKEN_BEDROCK",
    "AWS_PROFILE",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AZURE_OPENAI_API_KEY",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_VERTEX",
    "CODEX_API_KEY",
    "GOOGLE_APPLICATION_CREDENTIALS",
    "KIRO_API_KEY",
    "OPENAI_API_KEY",
    "OPENAI_BASE_URL",
];

/// Claims retained by the native qualification workflow for one executable.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each independently derived native qualification assertion must fail closed"
)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderQualification {
    pub schema: String,
    pub provider: ProviderKind,
    pub executable_digest: String,
    pub version: String,
    pub operating_system: String,
    pub architecture: String,
    pub evidence_sha256: String,
    pub evidence_path: String,
    pub receipt_manifest_sha256: String,
    pub subscription_login: bool,
    pub explicit_model: bool,
    pub explicit_effort: bool,
    pub session_reuse: bool,
    pub reports_effective_configuration: bool,
    pub delegation_contained: bool,
    pub native_cancellation_qualified: bool,
    pub resource_confinement_qualified: bool,
    pub selections: Vec<QualifiedSelection>,
}

impl ProviderQualification {
    /// Decodes bounded evidence before it is installed into protected state.
    ///
    /// # Errors
    /// Rejects malformed, oversized, incomplete, or failed evidence.
    pub fn decode(bytes: &[u8]) -> Result<Self, QualificationError> {
        if bytes.is_empty()
            || u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_QUALIFICATION_BYTES
        {
            return Err(QualificationError::InvalidRecord);
        }
        let record: Self =
            serde_json::from_slice(bytes).map_err(|_| QualificationError::InvalidRecord)?;
        record.validate()?;
        Ok(record)
    }

    /// Loads a bounded owner-only record. Qualification records live with
    /// protected application state, never in a provider install directory.
    ///
    /// # Errors
    /// Rejects public, redirected, oversized, malformed, or incomplete files.
    pub fn load(path: &Path) -> Result<Self, QualificationError> {
        validate_owner_only_file(path).map_err(|_| QualificationError::UnsafeRecord)?;
        let metadata = fs::metadata(path).map_err(|_| QualificationError::UnsafeRecord)?;
        if metadata.len() == 0 || metadata.len() > MAX_QUALIFICATION_BYTES {
            return Err(QualificationError::InvalidRecord);
        }
        Self::decode(&fs::read(path).map_err(|_| QualificationError::UnsafeRecord)?)
    }

    /// Installs the validated record as a new owner-only file. Existing
    /// evidence is never overwritten implicitly.
    ///
    /// # Errors
    /// Rejects invalid records, unsafe parent storage, collisions, or I/O loss.
    pub fn store(&self, path: &Path) -> Result<(), QualificationError> {
        self.validate()?;
        store_owner_only(self, path)
    }

    /// Applies evidence to a static inspection for one exact requested
    /// selection. Evidence may establish a control not listed by generic help,
    /// but cannot enable an adapter that rejected the installed version and
    /// never changes the discovered executable identity.
    ///
    /// # Errors
    /// Rejects stale evidence, another platform/provider, an unqualified
    /// setting pair, or any failed safety assertion.
    pub fn apply(
        &self,
        inspection: ProviderInspection,
        model: &str,
        effort: Option<&str>,
    ) -> Result<ProviderInspection, QualificationError> {
        self.validate()?;
        let selection = self
            .selections
            .iter()
            .find(|selection| selection.model == model && selection.effort.as_deref() == effort)
            .cloned()
            .ok_or(QualificationError::SelectionUnqualified)?;
        self.apply_selections(inspection, vec![selection])
    }

    /// Applies every exact selection retained by this native record.
    ///
    /// This is used only for an installed local binding. Provider preparation
    /// still checks each invocation against the retained set, so qualifying
    /// several pairs never creates a wildcard model capability.
    ///
    /// # Errors
    /// Rejects stale evidence, another platform/provider, or a static adapter
    /// contract that remains unsupported.
    pub fn apply_all(
        &self,
        inspection: ProviderInspection,
    ) -> Result<ProviderInspection, QualificationError> {
        self.apply_selections(inspection, self.selections.clone())
    }

    fn apply_selections(
        &self,
        mut inspection: ProviderInspection,
        selections: Vec<QualifiedSelection>,
    ) -> Result<ProviderInspection, QualificationError> {
        self.validate()?;
        let capabilities = inspection
            .capabilities
            .as_mut()
            .ok_or(QualificationError::StaticInspectionBlocked)?;
        if inspection.provider != self.provider
            || capabilities.provider != self.provider
            || capabilities.executable_digest != self.executable_digest
            || capabilities.version != self.version
        {
            return Err(QualificationError::IdentityMismatch);
        }
        if inspection.blocker == Some(ProviderBlocker::UnsupportedVersion) {
            return Err(QualificationError::StaticInspectionBlocked);
        }
        if self.operating_system != std::env::consts::OS
            || !architecture_matches(&self.architecture, std::env::consts::ARCH)
        {
            return Err(QualificationError::PlatformMismatch);
        }
        if !self.explicit_model
            || (selections
                .iter()
                .any(|selection| selection.effort.is_some())
                && !self.explicit_effort)
        {
            return Err(QualificationError::StaticInspectionBlocked);
        }
        capabilities.explicit_model = self.explicit_model;
        capabilities.explicit_effort = self.explicit_effort;
        capabilities.session_reuse = self.session_reuse;
        capabilities.reports_effective_configuration = true;
        capabilities.delegation_contained = true;
        capabilities.native_cancellation_qualified = true;
        capabilities.resource_confinement_qualified = true;
        capabilities.qualification = Some(ProviderQualificationBinding {
            provider: self.provider,
            executable_digest: self.executable_digest.clone(),
            version: self.version.clone(),
            evidence_sha256: self.evidence_sha256.clone(),
            operating_system: self.operating_system.clone(),
            architecture: self.architecture.clone(),
            resource_confinement_qualified: true,
            // v2 evidence predates the bidirectional Codex transport. It must
            // never authorize that changed invocation on CLI identity alone.
            // A future evidence-schema revision must derive this proof from
            // its native receipts, rather than a caller-supplied boolean.
            invocation_contract: None,
            local_codex_profile: None,
            selections,
        });
        inspection.blocker =
            capabilities.first_blocker(capabilities.qualification.as_ref().is_some_and(
                |binding| binding.selections.iter().any(|item| item.effort.is_some()),
            ));
        if inspection.blocker.is_some() {
            return Err(QualificationError::StaticInspectionBlocked);
        }
        Ok(inspection)
    }

    fn validate(&self) -> Result<(), QualificationError> {
        if self.schema != QUALIFICATION_SCHEMA
            || !bounded(&self.version)
            || !bounded(&self.operating_system)
            || !bounded(&self.architecture)
            || !valid_digest(&self.executable_digest, "blake3:")
            || !valid_digest(&self.evidence_sha256, "")
            || !safe_relative_path(&self.evidence_path)
            || !valid_digest(&self.receipt_manifest_sha256, "")
            || !self.subscription_login
            || !self.explicit_model
            || (self.provider != ProviderKind::Controlled && !self.explicit_effort)
            || !self.reports_effective_configuration
            || !self.delegation_contained
            || !self.native_cancellation_qualified
            || !self.resource_confinement_qualified
            || self.selections.is_empty()
            || self.selections.len() > MAX_SELECTIONS
            || self.selections.iter().any(|selection| {
                !bounded(&selection.model)
                    || (self.provider != ProviderKind::Controlled && selection.effort.is_none())
                    || selection
                        .effort
                        .as_deref()
                        .is_some_and(|value| !bounded(value))
            })
        {
            return Err(QualificationError::InvalidRecord);
        }
        if self
            .selections
            .iter()
            .enumerate()
            .any(|(index, selection)| self.selections[index + 1..].contains(selection))
        {
            return Err(QualificationError::InvalidRecord);
        }
        Ok(())
    }

    fn verify_evidence_bundle(&self, record_path: &Path) -> Result<(), QualificationError> {
        if self.provider == ProviderKind::Controlled {
            return Ok(());
        }
        let base = record_path
            .parent()
            .ok_or(QualificationError::UnsafeEvidence)?
            .canonicalize()
            .map_err(|_| QualificationError::UnsafeEvidence)?;
        let evidence_path = resolve_regular_bundle_file(&base, &self.evidence_path)?;
        let evidence_bytes = read_bounded_regular(&evidence_path)?;
        if sha256_hex(&evidence_bytes) != self.evidence_sha256 {
            return Err(QualificationError::EvidenceMismatch);
        }
        let evidence: QualificationEvidence = serde_json::from_slice(&evidence_bytes)
            .map_err(|_| QualificationError::InvalidEvidence)?;
        evidence.verify(self, &evidence_path)
    }
}

#[allow(
    dead_code,
    reason = "the exact evidence envelope rejects unknown fields even when a field is not a provider capability"
)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QualificationEvidence {
    schema: String,
    run_id: String,
    recorded_at: String,
    git_revision: String,
    receipt_manifest: BTreeMap<String, ReceiptManifestEntry>,
    package: EvidencePackage,
    platform: EvidencePlatform,
    coordination_engine: EvidenceEngine,
    roster_size: u64,
    execution: serde_json::Value,
    providers: Vec<EvidenceProvider>,
    scenarios: serde_json::Value,
    artifacts: serde_json::Value,
    status: String,
}

#[allow(
    dead_code,
    reason = "the exact evidence envelope rejects unknown fields beyond the provider binding subset"
)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidencePackage {
    sha256: String,
    target: String,
    application_version: String,
    install_schema: String,
    install_sha256: String,
    install_ref: String,
    pack_id: String,
    pack_version: String,
    pack_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidencePlatform {
    os: String,
    arch: String,
    native: bool,
}

#[allow(
    dead_code,
    reason = "the Pack digest is retained in the exact envelope but provider binding uses the engine identity"
)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceEngine {
    application: String,
    instance_id: String,
    pack_digest: String,
}

#[allow(
    dead_code,
    reason = "summary fields are parsed to reject schema drift; capability booleans are independently derived"
)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceProvider {
    provider: ProviderKind,
    status: String,
    reason: String,
    evidence_refs: Vec<String>,
    cli_version: Option<String>,
    executable_blake3: Option<String>,
    explicit_model: bool,
    explicit_effort: bool,
    session_reuse: bool,
    member_count: u64,
    effective_cap: u64,
    maximum_simultaneous_invocations: u64,
    selections: Vec<QualifiedSelection>,
    assertions: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiptManifestEntry {
    path: String,
    sha256: String,
    schema: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InvocationReceipt {
    schema: String,
    run_id: String,
    target: String,
    package_sha256: String,
    install_sha256: String,
    engine_instance_id: String,
    provider: ProviderKind,
    cli_version: String,
    executable_blake3: String,
    member_key: String,
    invocation_id: String,
    configuration_revision: u64,
    started_at_ms: u64,
    finished_at_ms: u64,
    requested: RequestedConfiguration,
    reported: Option<ReportedConfiguration>,
    proposal_schema: Option<String>,
    coordinator_disposition: String,
    submitted_action_id: Option<String>,
    blocker: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestedConfiguration {
    model: String,
    effort: Option<String>,
    session: RequestedSession,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestedSession {
    mode: String,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportedConfiguration {
    model: String,
    effort: Option<String>,
    session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginReceipt {
    schema: String,
    run_id: String,
    target: String,
    package_sha256: String,
    install_sha256: String,
    engine_instance_id: String,
    provider: ProviderKind,
    cli_version: String,
    executable_blake3: String,
    login_mode: String,
    removed_environment: Vec<String>,
    exit_code: i64,
    result_sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProcessReceipt {
    schema: String,
    run_id: String,
    target: String,
    package_sha256: String,
    install_sha256: String,
    engine_instance_id: String,
    provider: ProviderKind,
    invocation_id: String,
    owned_process_ids: Vec<String>,
    delegated_process_ids: Vec<String>,
    escaped_process_ids: Vec<String>,
    terminated_process_ids: Vec<String>,
    cancel_outcome: String,
    unrelated_process_id: Option<String>,
    unrelated_process_status: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResourceConfinementReceipt {
    schema: String,
    run_id: String,
    target: String,
    package_sha256: String,
    install_sha256: String,
    engine_instance_id: String,
    provider: ProviderKind,
    cli_version: String,
    executable_blake3: String,
    invocation_id: String,
    selection: QualifiedSelection,
    working_area: String,
    resource_policy: String,
    allowed_tools: Vec<String>,
    enforcement_layer: String,
    in_root_probe: FilesystemProbe,
    escape_probe: FilesystemProbe,
    unauthorized_tool_probe: ToolProbe,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FilesystemProbe {
    path: String,
    outcome: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolProbe {
    tool: String,
    outcome: String,
}

#[derive(Clone, Debug)]
struct AcceptedInvocation {
    member_key: String,
    invocation_id: String,
    revision: u64,
    started_at_ms: u64,
    finished_at_ms: u64,
    selection: QualifiedSelection,
    session_mode: String,
    session_id: Option<String>,
}

impl QualificationEvidence {
    fn verify(
        &self,
        qualification: &ProviderQualification,
        evidence_path: &Path,
    ) -> Result<(), QualificationError> {
        if self.schema != EVIDENCE_SCHEMA
            || self.status != "pass"
            || !bounded(&self.run_id)
            || !self.platform.native
            || self.platform.os != qualification.operating_system
            || self.platform.architecture() != qualification.architecture
            || self.coordination_engine.application != COORDINATION_ENGINE
            || !bounded(&self.coordination_engine.instance_id)
            || !valid_digest(&self.package.sha256, "")
            || !valid_digest(&self.package.install_sha256, "")
            || self.receipt_manifest.is_empty()
            || self.receipt_manifest.len() > MAX_EVIDENCE_RECEIPTS
            || manifest_sha256(&self.receipt_manifest) != qualification.receipt_manifest_sha256
        {
            return Err(QualificationError::InvalidEvidence);
        }
        let base = evidence_path
            .parent()
            .ok_or(QualificationError::UnsafeEvidence)?;
        let mut receipt_documents = BTreeMap::new();
        for (receipt_id, entry) in &self.receipt_manifest {
            if !bounded(receipt_id)
                || !safe_relative_path(&entry.path)
                || !valid_digest(&entry.sha256, "")
                || receipt_kind_for_schema(&entry.schema) != Some(entry.kind.as_str())
            {
                return Err(QualificationError::InvalidEvidence);
            }
            let path = resolve_regular_bundle_file(base, &entry.path)?;
            let bytes = read_bounded_regular(&path)?;
            if sha256_hex(&bytes) != entry.sha256 {
                return Err(QualificationError::EvidenceMismatch);
            }
            let document: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|_| QualificationError::InvalidEvidence)?;
            if document.get("schema").and_then(serde_json::Value::as_str)
                != Some(entry.schema.as_str())
            {
                return Err(QualificationError::InvalidEvidence);
            }
            receipt_documents.insert(receipt_id.clone(), bytes);
        }
        let provider = self
            .providers
            .iter()
            .find(|provider| provider.provider == qualification.provider)
            .ok_or(QualificationError::InvalidEvidence)?;
        self.verify_provider(qualification, provider, &receipt_documents)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one closed verifier derives every provider capability from the same receipt set"
    )]
    fn verify_provider(
        &self,
        qualification: &ProviderQualification,
        provider: &EvidenceProvider,
        receipt_documents: &BTreeMap<String, Vec<u8>>,
    ) -> Result<(), QualificationError> {
        if provider.status != "pass"
            || provider.cli_version.as_deref() != Some(qualification.version.as_str())
            || provider.executable_blake3.as_deref()
                != Some(qualification.executable_digest.as_str())
            || !provider.explicit_model
            || !provider.explicit_effort
            || !provider.session_reuse
            || provider.evidence_refs.is_empty()
            || provider.evidence_refs.len() > MAX_EVIDENCE_RECEIPTS
        {
            return Err(QualificationError::InvalidEvidence);
        }
        let unique_refs = provider.evidence_refs.iter().collect::<BTreeSet<&String>>();
        if unique_refs.len() != provider.evidence_refs.len() {
            return Err(QualificationError::InvalidEvidence);
        }

        let mut accepted = Vec::new();
        let mut unavailable_blocked = false;
        let mut login_qualified = false;
        let mut process_qualified = false;
        let mut confined_selections = BTreeSet::new();
        for receipt_id in &provider.evidence_refs {
            let entry = self
                .receipt_manifest
                .get(receipt_id)
                .ok_or(QualificationError::InvalidEvidence)?;
            let bytes = receipt_documents
                .get(receipt_id)
                .ok_or(QualificationError::InvalidEvidence)?;
            match entry.schema.as_str() {
                INVOCATION_RECEIPT_SCHEMA => {
                    let receipt: InvocationReceipt = serde_json::from_slice(bytes)
                        .map_err(|_| QualificationError::InvalidEvidence)?;
                    validate_receipt_binding(
                        &receipt.schema,
                        &receipt.run_id,
                        &receipt.target,
                        &receipt.package_sha256,
                        &receipt.install_sha256,
                        &receipt.engine_instance_id,
                        INVOCATION_RECEIPT_SCHEMA,
                        self,
                    )?;
                    if receipt.provider != qualification.provider
                        || receipt.cli_version != qualification.version
                        || receipt.executable_blake3 != qualification.executable_digest
                        || !bounded(&receipt.member_key)
                        || !bounded(&receipt.invocation_id)
                        || receipt.configuration_revision == 0
                        || receipt.started_at_ms >= receipt.finished_at_ms
                        || !bounded(&receipt.requested.model)
                        || receipt
                            .requested
                            .effort
                            .as_deref()
                            .is_none_or(|effort| !bounded(effort))
                        || !matches!(receipt.requested.session.mode.as_str(), "fresh" | "resume")
                        || receipt
                            .requested
                            .session
                            .session_id
                            .as_deref()
                            .is_some_and(|session| !bounded(session))
                        || (receipt.requested.session.mode == "resume"
                            && receipt.requested.session.session_id.is_none())
                    {
                        return Err(QualificationError::InvalidEvidence);
                    }
                    if receipt.coordinator_disposition == "submitted" {
                        let reported = receipt
                            .reported
                            .as_ref()
                            .ok_or(QualificationError::InvalidEvidence)?;
                        if receipt.proposal_schema.as_deref() != Some(ACTION_PROPOSAL_SCHEMA)
                            || receipt
                                .submitted_action_id
                                .as_deref()
                                .is_none_or(|action| !bounded(action))
                            || receipt.blocker.is_some()
                            || reported.model != receipt.requested.model
                            || reported.effort != receipt.requested.effort
                            || !reported_session_matches(
                                reported.session_id.as_deref(),
                                &receipt.requested.session,
                            )
                        {
                            return Err(QualificationError::InvalidEvidence);
                        }
                        accepted.push(AcceptedInvocation {
                            member_key: receipt.member_key,
                            invocation_id: receipt.invocation_id,
                            revision: receipt.configuration_revision,
                            started_at_ms: receipt.started_at_ms,
                            finished_at_ms: receipt.finished_at_ms,
                            selection: QualifiedSelection {
                                model: receipt.requested.model,
                                effort: receipt.requested.effort,
                            },
                            session_mode: receipt.requested.session.mode,
                            session_id: reported.session_id.clone(),
                        });
                    } else if receipt.coordinator_disposition == "not_submitted"
                        && receipt.reported.is_none()
                        && receipt.proposal_schema.is_none()
                        && receipt.submitted_action_id.is_none()
                        && receipt.blocker.as_deref().is_some_and(bounded)
                    {
                        unavailable_blocked = true;
                    } else {
                        return Err(QualificationError::InvalidEvidence);
                    }
                }
                LOGIN_RECEIPT_SCHEMA => {
                    if login_qualified {
                        return Err(QualificationError::InvalidEvidence);
                    }
                    let receipt: LoginReceipt = serde_json::from_slice(bytes)
                        .map_err(|_| QualificationError::InvalidEvidence)?;
                    validate_receipt_binding(
                        &receipt.schema,
                        &receipt.run_id,
                        &receipt.target,
                        &receipt.package_sha256,
                        &receipt.install_sha256,
                        &receipt.engine_instance_id,
                        LOGIN_RECEIPT_SCHEMA,
                        self,
                    )?;
                    let removed = receipt
                        .removed_environment
                        .iter()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>();
                    login_qualified = receipt.provider == qualification.provider
                        && receipt.cli_version == qualification.version
                        && receipt.executable_blake3 == qualification.executable_digest
                        && receipt.login_mode == "normal_subscription_cli"
                        && receipt.exit_code == 0
                        && valid_digest(&receipt.result_sha256, "")
                        && DIRECT_API_ENVIRONMENT
                            .iter()
                            .all(|name| removed.contains(name));
                }
                PROCESS_RECEIPT_SCHEMA => {
                    if process_qualified {
                        return Err(QualificationError::InvalidEvidence);
                    }
                    let receipt: ProcessReceipt = serde_json::from_slice(bytes)
                        .map_err(|_| QualificationError::InvalidEvidence)?;
                    validate_receipt_binding(
                        &receipt.schema,
                        &receipt.run_id,
                        &receipt.target,
                        &receipt.package_sha256,
                        &receipt.install_sha256,
                        &receipt.engine_instance_id,
                        PROCESS_RECEIPT_SCHEMA,
                        self,
                    )?;
                    let owned = bounded_set(&receipt.owned_process_ids)?;
                    let delegated = bounded_set(&receipt.delegated_process_ids)?;
                    let escaped = bounded_set(&receipt.escaped_process_ids)?;
                    let terminated = bounded_set(&receipt.terminated_process_ids)?;
                    process_qualified = receipt.provider == qualification.provider
                        && bounded(&receipt.invocation_id)
                        && !owned.is_empty()
                        // Agent Swarm has no scheduler accounting seam for a
                        // provider-created subagent. Until one exists, native
                        // delegation must be disabled rather than merely kept
                        // inside the owned process tree: ownership proves
                        // cleanup, not that the delegated work consumed a
                        // roster/provider-cap slot.
                        && delegated.is_empty()
                        && escaped.is_empty()
                        && terminated == owned
                        && receipt.cancel_outcome == "terminated"
                        && receipt.unrelated_process_id.as_deref().is_some_and(bounded)
                        && receipt.unrelated_process_status == "running";
                }
                RESOURCE_CONFINEMENT_RECEIPT_SCHEMA => {
                    let receipt: ResourceConfinementReceipt = serde_json::from_slice(bytes)
                        .map_err(|_| QualificationError::InvalidEvidence)?;
                    validate_receipt_binding(
                        &receipt.schema,
                        &receipt.run_id,
                        &receipt.target,
                        &receipt.package_sha256,
                        &receipt.install_sha256,
                        &receipt.engine_instance_id,
                        RESOURCE_CONFINEMENT_RECEIPT_SCHEMA,
                        self,
                    )?;
                    let working_area = Path::new(&receipt.working_area);
                    let in_root = Path::new(&receipt.in_root_probe.path);
                    let escape = Path::new(&receipt.escape_probe.path);
                    let allowed_tools = bounded_set(&receipt.allowed_tools)?;
                    if receipt.provider != qualification.provider
                        || receipt.cli_version != qualification.version
                        || receipt.executable_blake3 != qualification.executable_digest
                        || !bounded(&receipt.invocation_id)
                        || !working_area.is_absolute()
                        || !normalized_absolute_path(working_area)
                        || !in_root.is_absolute()
                        || !normalized_absolute_path(in_root)
                        || !in_root.starts_with(working_area)
                        || !escape.is_absolute()
                        || !normalized_absolute_path(escape)
                        || escape.starts_with(working_area)
                        || receipt.in_root_probe.outcome != "allowed"
                        || receipt.escape_probe.outcome != "denied"
                        || !bounded(&receipt.unauthorized_tool_probe.tool)
                        || allowed_tools.contains(receipt.unauthorized_tool_probe.tool.as_str())
                        || receipt.unauthorized_tool_probe.outcome != "denied"
                        || receipt.enforcement_layer != "worldstream-agent-swarm-process-guard"
                        || !matches!(
                            receipt.resource_policy.as_str(),
                            "read_only" | "workspace_write"
                        )
                        || !bounded(&receipt.selection.model)
                        || receipt
                            .selection
                            .effort
                            .as_deref()
                            .is_none_or(|effort| !bounded(effort))
                    {
                        return Err(QualificationError::InvalidEvidence);
                    }
                    if !confined_selections
                        .insert((receipt.selection.model, receipt.selection.effort))
                    {
                        return Err(QualificationError::InvalidEvidence);
                    }
                }
                _ => return Err(QualificationError::InvalidEvidence),
            }
        }

        let derived_selections = accepted
            .iter()
            .map(|invocation| {
                (
                    invocation.selection.model.clone(),
                    invocation.selection.effort.clone(),
                )
            })
            .collect::<BTreeSet<_>>();
        let recorded_selections = qualification
            .selections
            .iter()
            .map(|selection| (selection.model.clone(), selection.effort.clone()))
            .collect::<BTreeSet<_>>();
        let summary_selections = provider
            .selections
            .iter()
            .map(|selection| (selection.model.clone(), selection.effort.clone()))
            .collect::<BTreeSet<_>>();
        let isolated = accepted.iter().enumerate().any(|(index, left)| {
            accepted[index + 1..].iter().any(|right| {
                left.member_key != right.member_key
                    && left.selection != right.selection
                    && left.started_at_ms < right.finished_at_ms
                    && right.started_at_ms < left.finished_at_ms
            })
        });
        let reused_session = accepted.iter().any(|first| {
            accepted.iter().any(|second| {
                first.member_key == second.member_key
                    && first.revision.checked_add(1) == Some(second.revision)
                    && second.session_mode == "resume"
                    && first.session_id.is_some()
                    && first.session_id == second.session_id
                    && first.invocation_id != second.invocation_id
            })
        });
        let next_invocation_setting_change = accepted.iter().any(|first| {
            accepted.iter().any(|second| {
                first.member_key == second.member_key
                    && first.revision.checked_add(1) == Some(second.revision)
                    && second.started_at_ms >= first.finished_at_ms
                    && first.selection != second.selection
                    && first.invocation_id != second.invocation_id
            })
        });
        if derived_selections.len() < 2
            || derived_selections != recorded_selections
            || derived_selections != summary_selections
            || !isolated
            || !reused_session
            || !next_invocation_setting_change
            || !unavailable_blocked
            || !login_qualified
            || !process_qualified
            || confined_selections != derived_selections
            || !qualification.subscription_login
            || !qualification.explicit_model
            || !qualification.explicit_effort
            || !qualification.session_reuse
            || !qualification.reports_effective_configuration
            || !qualification.delegation_contained
            || !qualification.native_cancellation_qualified
            || !qualification.resource_confinement_qualified
        {
            return Err(QualificationError::InvalidEvidence);
        }
        Ok(())
    }
}

fn reported_session_matches(reported: Option<&str>, requested: &RequestedSession) -> bool {
    match (requested.mode.as_str(), requested.session_id.as_deref()) {
        ("fresh", None) => reported.is_some_and(bounded),
        ("fresh" | "resume", Some(requested)) => reported == Some(requested),
        _ => false,
    }
}

impl EvidencePlatform {
    fn architecture(&self) -> &str {
        match self.arch.as_str() {
            "arm64" => "arm64",
            "x86_64" => "x86_64",
            other => other,
        }
    }
}

/// Owner-protected local binding between native evidence and one exact path.
///
/// Portable evidence intentionally does not authorize whichever executable a
/// later caller supplies. Installation canonicalizes the portable record and
/// selected executable; every load re-hashes the source evidence graph and
/// repeats the executable proof before returning process-capable data.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledProviderQualification {
    schema: String,
    executable: PathBuf,
    qualification_record: PathBuf,
    qualification: ProviderQualification,
}

impl InstalledProviderQualification {
    /// Binds portable native evidence to one exact executable installation.
    ///
    /// # Errors
    /// Rejects an unavailable or unsupported executable, identity/version
    /// mismatch, another native platform, or incomplete evidence.
    pub fn bind(
        qualification_record: &Path,
        executable: &Path,
    ) -> Result<Self, QualificationError> {
        let record_metadata = fs::symlink_metadata(qualification_record)
            .map_err(|_| QualificationError::UnsafeRecord)?;
        if record_metadata.file_type().is_symlink()
            || !record_metadata.is_file()
            || record_metadata.len() == 0
            || record_metadata.len() > MAX_QUALIFICATION_BYTES
        {
            return Err(QualificationError::UnsafeRecord);
        }
        let qualification = ProviderQualification::decode(
            &fs::read(qualification_record).map_err(|_| QualificationError::UnsafeRecord)?,
        )?;
        qualification.verify_evidence_bundle(qualification_record)?;
        let qualification_record = qualification_record
            .canonicalize()
            .map_err(|_| QualificationError::UnsafeRecord)?;
        let inspection = ProviderRegistry::new().inspect(
            qualification.provider,
            &NativeProviderProbe::default(),
            executable,
        );
        let qualified = qualification.apply_all(inspection)?;
        let executable = qualified
            .capabilities
            .ok_or(QualificationError::StaticInspectionBlocked)?
            .executable;
        Ok(Self {
            schema: INSTALLED_QUALIFICATION_SCHEMA.to_owned(),
            executable,
            qualification_record,
            qualification,
        })
    }

    /// Loads a bounded owner-only installed binding and rechecks the portable
    /// evidence graph, current executable bytes, reported version, platform,
    /// and full selection set.
    ///
    /// # Errors
    /// Rejects unsafe storage, malformed bindings, or a changed executable.
    pub fn load(path: &Path) -> Result<Self, QualificationError> {
        validate_owner_only_file(path).map_err(|_| QualificationError::UnsafeRecord)?;
        let metadata = fs::metadata(path).map_err(|_| QualificationError::UnsafeRecord)?;
        if metadata.len() == 0 || metadata.len() > MAX_QUALIFICATION_BYTES {
            return Err(QualificationError::InvalidRecord);
        }
        let installed: Self =
            serde_json::from_slice(&fs::read(path).map_err(|_| QualificationError::UnsafeRecord)?)
                .map_err(|_| QualificationError::InvalidRecord)?;
        installed.capabilities()?;
        Ok(installed)
    }

    /// Installs a validated binding as a new owner-only file.
    ///
    /// # Errors
    /// Rejects unsafe parent storage, collisions, changed executables, or I/O
    /// loss. Existing bindings are never overwritten implicitly.
    pub fn store(&self, path: &Path) -> Result<(), QualificationError> {
        self.capabilities()?;
        store_owner_only(self, path)
    }

    /// Re-probes the bound executable and returns only capabilities carrying
    /// the exact qualified model/effort set.
    ///
    /// # Errors
    /// Rejects malformed bindings and any executable/version/platform drift.
    pub fn capabilities(&self) -> Result<ProviderCapabilities, QualificationError> {
        if self.schema != INSTALLED_QUALIFICATION_SCHEMA
            || !self.executable.is_absolute()
            || !self.qualification_record.is_absolute()
        {
            return Err(QualificationError::InvalidRecord);
        }
        let retained =
            ProviderQualification::decode(&read_bounded_regular(&self.qualification_record)?)?;
        if retained != self.qualification {
            return Err(QualificationError::EvidenceMismatch);
        }
        retained.verify_evidence_bundle(&self.qualification_record)?;
        let inspection = ProviderRegistry::new().inspect(
            self.qualification.provider,
            &NativeProviderProbe::default(),
            &self.executable,
        );
        let capabilities = self
            .qualification
            .apply_all(inspection)?
            .capabilities
            .ok_or(QualificationError::StaticInspectionBlocked)?;
        if capabilities.executable != self.executable {
            return Err(QualificationError::IdentityMismatch);
        }
        Ok(capabilities)
    }

    /// Re-probes the bound executable for one exact requested selection.
    ///
    /// # Errors
    /// Rejects executable drift or an unqualified model/effort pair.
    pub fn qualify(
        &self,
        model: &str,
        effort: Option<&str>,
    ) -> Result<ProviderCapabilities, QualificationError> {
        if self.schema != INSTALLED_QUALIFICATION_SCHEMA
            || !self.executable.is_absolute()
            || !self.qualification_record.is_absolute()
        {
            return Err(QualificationError::InvalidRecord);
        }
        let retained =
            ProviderQualification::decode(&read_bounded_regular(&self.qualification_record)?)?;
        if retained != self.qualification {
            return Err(QualificationError::EvidenceMismatch);
        }
        retained.verify_evidence_bundle(&self.qualification_record)?;
        let inspection = ProviderRegistry::new().inspect(
            self.qualification.provider,
            &NativeProviderProbe::default(),
            &self.executable,
        );
        let capabilities = self
            .qualification
            .apply(inspection, model, effort)?
            .capabilities
            .ok_or(QualificationError::StaticInspectionBlocked)?;
        if capabilities.executable != self.executable {
            return Err(QualificationError::IdentityMismatch);
        }
        Ok(capabilities)
    }
}

/// Closed qualification failures; evidence contents are never included.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum QualificationError {
    #[error("provider qualification record is unsafe")]
    UnsafeRecord,
    #[error("provider qualification record already exists or cannot be created")]
    RecordExists,
    #[error("provider qualification record is invalid or failed")]
    InvalidRecord,
    #[error("provider qualification evidence bundle is unsafe")]
    UnsafeEvidence,
    #[error("provider qualification evidence is malformed or incomplete")]
    InvalidEvidence,
    #[error("provider qualification evidence digest does not match")]
    EvidenceMismatch,
    #[error("provider qualification does not match the executable")]
    IdentityMismatch,
    #[error("provider qualification is for another native platform")]
    PlatformMismatch,
    #[error("requested provider selection was not qualified")]
    SelectionUnqualified,
    #[error("static provider inspection remains blocked")]
    StaticInspectionBlocked,
}

fn bounded(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= MAX_TEXT_BYTES && !value.contains('\0')
}

fn safe_relative_path(value: &str) -> bool {
    bounded(value)
        && !value.contains('\\')
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn normalized_absolute_path(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                Component::Prefix(_) | Component::RootDir | Component::Normal(_)
            )
        })
}

fn resolve_regular_bundle_file(base: &Path, relative: &str) -> Result<PathBuf, QualificationError> {
    if !safe_relative_path(relative) {
        return Err(QualificationError::UnsafeEvidence);
    }
    let mut candidate = base.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return Err(QualificationError::UnsafeEvidence);
        };
        candidate.push(component);
        let metadata =
            fs::symlink_metadata(&candidate).map_err(|_| QualificationError::UnsafeEvidence)?;
        if metadata.file_type().is_symlink() {
            return Err(QualificationError::UnsafeEvidence);
        }
    }
    let resolved = candidate
        .canonicalize()
        .map_err(|_| QualificationError::UnsafeEvidence)?;
    if !resolved.starts_with(base) {
        return Err(QualificationError::UnsafeEvidence);
    }
    let metadata = fs::metadata(&resolved).map_err(|_| QualificationError::UnsafeEvidence)?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_QUALIFICATION_BYTES {
        return Err(QualificationError::UnsafeEvidence);
    }
    Ok(resolved)
}

fn read_bounded_regular(path: &Path) -> Result<Vec<u8>, QualificationError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| QualificationError::UnsafeEvidence)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_QUALIFICATION_BYTES
    {
        return Err(QualificationError::UnsafeEvidence);
    }
    fs::read(path).map_err(|_| QualificationError::UnsafeEvidence)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn manifest_sha256(manifest: &BTreeMap<String, ReceiptManifestEntry>) -> String {
    let mut digest = Sha256::new();
    for (receipt_id, entry) in manifest {
        for value in [
            receipt_id.as_str(),
            entry.path.as_str(),
            entry.sha256.as_str(),
            entry.schema.as_str(),
            entry.kind.as_str(),
        ] {
            digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
            digest.update(value.as_bytes());
        }
    }
    format!("{:x}", digest.finalize())
}

fn receipt_kind_for_schema(schema: &str) -> Option<&'static str> {
    match schema {
        INSTALL_RECEIPT_SCHEMA => Some("install"),
        INVOCATION_RECEIPT_SCHEMA => Some("provider_invocation"),
        LOGIN_RECEIPT_SCHEMA => Some("provider_login"),
        SCHEDULER_RECEIPT_SCHEMA => Some("scheduler_trace"),
        PROCESS_RECEIPT_SCHEMA => Some("process_containment"),
        RESOURCE_CONFINEMENT_RECEIPT_SCHEMA => Some("resource_confinement"),
        SCENARIO_RECEIPT_SCHEMA => Some("scenario"),
        _ => None,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "all content-addressed receipt bindings are compared at one closed validation seam"
)]
fn validate_receipt_binding(
    schema: &str,
    run_id: &str,
    target: &str,
    package_sha256: &str,
    install_sha256: &str,
    engine_instance_id: &str,
    expected_schema: &str,
    evidence: &QualificationEvidence,
) -> Result<(), QualificationError> {
    if schema != expected_schema
        || run_id != evidence.run_id
        || target != evidence.package.target
        || package_sha256 != evidence.package.sha256
        || install_sha256 != evidence.package.install_sha256
        || engine_instance_id != evidence.coordination_engine.instance_id
    {
        return Err(QualificationError::InvalidEvidence);
    }
    Ok(())
}

fn bounded_set(values: &[String]) -> Result<BTreeSet<&str>, QualificationError> {
    if values.len() > 1024 || values.iter().any(|value| !bounded(value)) {
        return Err(QualificationError::InvalidEvidence);
    }
    let set = values.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if set.len() != values.len() {
        return Err(QualificationError::InvalidEvidence);
    }
    Ok(set)
}

fn valid_digest(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn architecture_matches(evidence: &str, native: &str) -> bool {
    evidence == native
        || matches!(
            (evidence, native),
            ("arm64", "aarch64") | ("amd64", "x86_64")
        )
}

fn store_owner_only(value: &impl Serialize, path: &Path) -> Result<(), QualificationError> {
    let parent = path.parent().ok_or(QualificationError::UnsafeRecord)?;
    let parent = prepare_data_directory(parent).map_err(|_| QualificationError::UnsafeRecord)?;
    let file_name = path.file_name().ok_or(QualificationError::UnsafeRecord)?;
    let path = parent.join(file_name);
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| QualificationError::InvalidRecord)?;
    let mut file = create_owner_only_file(&path).map_err(|_| QualificationError::RecordExists)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|_| QualificationError::UnsafeRecord)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, error::Error};

    use serde_json::{Value, json};

    use super::*;

    fn bound_receipt(schema: &str, mut payload: Value) -> Value {
        let Some(object) = payload.as_object_mut() else {
            return Value::Object(serde_json::Map::new());
        };
        for (key, value) in [
            ("schema", json!(schema)),
            ("run_id", json!("native-run")),
            ("target", json!("native-target")),
            ("package_sha256", json!("1".repeat(64))),
            ("install_sha256", json!("2".repeat(64))),
            ("engine_instance_id", json!("engine-a")),
        ] {
            object.insert(key.to_owned(), value);
        }
        payload
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the test helper names every independently validated invocation receipt field"
    )]
    fn invocation(
        id: &str,
        member: &str,
        revision: u64,
        model: &str,
        effort: &str,
        start: u64,
        finish: u64,
        session_mode: &str,
        session_id: &str,
    ) -> Value {
        bound_receipt(
            INVOCATION_RECEIPT_SCHEMA,
            json!({
                "provider": "codex",
                "cli_version": "codex-cli 1",
                "executable_blake3": format!("blake3:{}", "a".repeat(64)),
                "member_key": member,
                "invocation_id": id,
                "configuration_revision": revision,
                "started_at_ms": start,
                "finished_at_ms": finish,
                "requested": {
                    "model": model,
                    "effort": effort,
                    "session": {"mode": session_mode, "session_id": session_id},
                },
                "reported": {"model": model, "effort": effort, "session_id": session_id},
                "proposal_schema": ACTION_PROPOSAL_SCHEMA,
                "coordinator_disposition": "submitted",
                "submitted_action_id": format!("action-{id}"),
                "blocker": null,
            }),
        )
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the fixture intentionally emits a complete typed qualification evidence graph"
    )]
    fn qualification_bundle(
        root: &Path,
        escape_outcome: &str,
    ) -> Result<(PathBuf, ProviderQualification), Box<dyn Error>> {
        let record_directory = prepare_data_directory(&root.join("portable"))?;
        let bundle = record_directory.join("bundle");
        let receipts_directory = bundle.join("receipts");
        fs::create_dir_all(&receipts_directory)?;
        let working_area = root.join("workspace");
        let escape_path = root.join("outside").join("probe.txt");
        let executable_digest = format!("blake3:{}", "a".repeat(64));
        let mut provider_generated_session = invocation(
            "inv-a",
            "member-a",
            1,
            "model-a",
            "low",
            100,
            200,
            "fresh",
            "session-a",
        );
        provider_generated_session["requested"]["session"]["session_id"] = Value::Null;
        let mut receipts = BTreeMap::from([
            ("invocation-a".to_owned(), provider_generated_session),
            (
                "invocation-b".to_owned(),
                invocation(
                    "inv-b",
                    "member-b",
                    1,
                    "model-b",
                    "high",
                    100,
                    200,
                    "fresh",
                    "session-b",
                ),
            ),
            (
                "invocation-next".to_owned(),
                invocation(
                    "inv-next",
                    "member-a",
                    2,
                    "model-a",
                    "low",
                    300,
                    400,
                    "resume",
                    "session-a",
                ),
            ),
            (
                "invocation-change".to_owned(),
                invocation(
                    "inv-change",
                    "member-a",
                    3,
                    "model-b",
                    "high",
                    410,
                    490,
                    "fresh",
                    "session-change",
                ),
            ),
            (
                "login".to_owned(),
                bound_receipt(
                    LOGIN_RECEIPT_SCHEMA,
                    json!({
                        "provider": "codex",
                        "cli_version": "codex-cli 1",
                        "executable_blake3": executable_digest,
                        "login_mode": "normal_subscription_cli",
                        "removed_environment": DIRECT_API_ENVIRONMENT,
                        "exit_code": 0,
                        "result_sha256": "3".repeat(64),
                    }),
                ),
            ),
            (
                "process".to_owned(),
                bound_receipt(
                    PROCESS_RECEIPT_SCHEMA,
                    json!({
                        "provider": "codex",
                        "invocation_id": "inv-a",
                        "owned_process_ids": ["parent", "child"],
                        "delegated_process_ids": [],
                        "escaped_process_ids": [],
                        "terminated_process_ids": ["child", "parent"],
                        "cancel_outcome": "terminated",
                        "unrelated_process_id": "unrelated",
                        "unrelated_process_status": "running",
                    }),
                ),
            ),
        ]);
        receipts.insert(
            "invocation-blocked".to_owned(),
            bound_receipt(
                INVOCATION_RECEIPT_SCHEMA,
                json!({
                    "provider": "codex",
                    "cli_version": "codex-cli 1",
                    "executable_blake3": executable_digest,
                    "member_key": "member-b",
                    "invocation_id": "inv-blocked",
                    "configuration_revision": 2,
                    "started_at_ms": 500,
                    "finished_at_ms": 510,
                    "requested": {
                        "model": "unavailable",
                        "effort": "maximum",
                        "session": {"mode": "fresh", "session_id": "blocked-session"},
                    },
                    "reported": null,
                    "proposal_schema": null,
                    "coordinator_disposition": "not_submitted",
                    "submitted_action_id": null,
                    "blocker": "unavailable_setting",
                }),
            ),
        );
        for (suffix, model, effort) in [("a", "model-a", "low"), ("b", "model-b", "high")] {
            receipts.insert(
                format!("confinement-{suffix}"),
                bound_receipt(
                    RESOURCE_CONFINEMENT_RECEIPT_SCHEMA,
                    json!({
                        "provider": "codex",
                        "cli_version": "codex-cli 1",
                        "executable_blake3": executable_digest,
                        "invocation_id": format!("confine-{suffix}"),
                        "selection": {"model": model, "effort": effort},
                        "working_area": working_area,
                        "resource_policy": "workspace_write",
                        "allowed_tools": [],
                        "enforcement_layer": "worldstream-agent-swarm-process-guard",
                        "in_root_probe": {
                            "path": working_area.join(format!("inside-{suffix}")),
                            "outcome": "allowed",
                        },
                        "escape_probe": {"path": escape_path, "outcome": escape_outcome},
                        "unauthorized_tool_probe": {"tool": "shell", "outcome": "denied"},
                    }),
                ),
            );
        }

        let mut manifest = serde_json::Map::new();
        let mut refs = Vec::new();
        for (receipt_id, receipt) in receipts {
            let bytes = serde_json::to_vec(&receipt)?;
            let relative = format!("receipts/{receipt_id}.json");
            fs::write(bundle.join(&relative), &bytes)?;
            let schema = receipt["schema"].as_str().ok_or("missing schema")?;
            manifest.insert(
                receipt_id.clone(),
                json!({
                    "path": relative,
                    "sha256": sha256_hex(&bytes),
                    "schema": schema,
                    "kind": receipt_kind_for_schema(schema).ok_or("unknown receipt")?,
                }),
            );
            refs.push(receipt_id);
        }
        let manifest_typed: BTreeMap<String, ReceiptManifestEntry> =
            serde_json::from_value(Value::Object(manifest.clone()))?;
        let evidence = json!({
            "schema": EVIDENCE_SCHEMA,
            "run_id": "native-run",
            "recorded_at": "2026-09-16T00:00:00Z",
            "git_revision": "f".repeat(40),
            "receipt_manifest": manifest,
            "package": {
                "sha256": "1".repeat(64),
                "target": "native-target",
                "application_version": "0.1.0",
                "install_schema": INSTALL_RECEIPT_SCHEMA,
                "install_sha256": "2".repeat(64),
                "install_ref": "install",
                "pack_id": "worldstream.agent-swarm",
                "pack_version": "0.2.0",
                "pack_digest": format!("blake3:{}", "4".repeat(64)),
            },
            "platform": {
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "native": true,
            },
            "coordination_engine": {
                "application": COORDINATION_ENGINE,
                "instance_id": "engine-a",
                "pack_digest": format!("blake3:{}", "4".repeat(64)),
            },
            "roster_size": 9,
            "execution": {},
            "providers": [{
                "provider": "codex",
                "status": "pass",
                "reason": "typed receipts",
                "evidence_refs": refs,
                "cli_version": "codex-cli 1",
                "executable_blake3": executable_digest,
                "explicit_model": true,
                "explicit_effort": true,
                "session_reuse": true,
                "member_count": 2,
                "effective_cap": 2,
                "maximum_simultaneous_invocations": 2,
                "selections": [
                    {"model": "model-a", "effort": "low"},
                    {"model": "model-b", "effort": "high"},
                ],
                "assertions": {},
            }],
            "scenarios": {},
            "artifacts": {},
            "status": "pass",
        });
        let evidence_bytes = serde_json::to_vec(&evidence)?;
        fs::write(bundle.join("evidence.json"), &evidence_bytes)?;
        let qualification = ProviderQualification {
            schema: QUALIFICATION_SCHEMA.to_owned(),
            provider: ProviderKind::Codex,
            executable_digest,
            version: "codex-cli 1".to_owned(),
            operating_system: std::env::consts::OS.to_owned(),
            architecture: std::env::consts::ARCH.to_owned(),
            evidence_sha256: sha256_hex(&evidence_bytes),
            evidence_path: "bundle/evidence.json".to_owned(),
            receipt_manifest_sha256: manifest_sha256(&manifest_typed),
            subscription_login: true,
            explicit_model: true,
            explicit_effort: true,
            session_reuse: true,
            reports_effective_configuration: true,
            delegation_contained: true,
            native_cancellation_qualified: true,
            resource_confinement_qualified: true,
            selections: vec![
                QualifiedSelection {
                    model: "model-a".to_owned(),
                    effort: Some("low".to_owned()),
                },
                QualifiedSelection {
                    model: "model-b".to_owned(),
                    effort: Some("high".to_owned()),
                },
            ],
        };
        let record_path = record_directory.join("qualification.json");
        qualification.store(&record_path)?;
        Ok((record_path, qualification))
    }

    #[test]
    fn portable_qualification_rederives_typed_receipts_and_escape_probe()
    -> Result<(), Box<dyn Error>> {
        let valid = tempfile::tempdir()?;
        let (record, qualification) = qualification_bundle(valid.path(), "denied")?;
        qualification.verify_evidence_bundle(&record)?;
        fs::write(
            valid.path().join("portable/bundle/receipts/login.json"),
            b"{}",
        )?;
        assert_eq!(
            qualification.verify_evidence_bundle(&record),
            Err(QualificationError::EvidenceMismatch)
        );

        let escaped = tempfile::tempdir()?;
        let (record, qualification) = qualification_bundle(escaped.path(), "allowed")?;
        assert_eq!(
            qualification.verify_evidence_bundle(&record),
            Err(QualificationError::InvalidEvidence)
        );
        Ok(())
    }
}

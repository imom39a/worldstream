#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

//! One provider-neutral scenario catalog and result schema for kernel parity.
//!
//! The scenario definitions in this crate are the comparison boundary. Every
//! adapter executes every definition through the same trait; an adapter error
//! fails the run instead of being converted into an incomplete parity row.

use serde::{Deserialize, Serialize};
use worldstream_core::CanonicalJsonV1;

/// The frozen acceptance rows for the full kernel lane.
pub const SCENARIOS: &[ScenarioDefinition] = &[
    ScenarioDefinition::new("create", "create, duplicate, conflict, receipt resolution"),
    ScenarioDefinition::new("core_admin", "Core administration transition and receipt"),
    ScenarioDefinition::new("action", "Participant Action transition and receipt"),
    ScenarioDefinition::new(
        "timer",
        "conditional TimerFired generation and payload witness",
    ),
    ScenarioDefinition::new(
        "frame_cursor_reset",
        "frames, Cursor, prune, reset and visibility",
    ),
    ScenarioDefinition::new(
        "activation",
        "offer, claim, renew, release, complete and leases",
    ),
    ScenarioDefinition::new(
        "recovery",
        "restart, snapshot/cache, replay and integrity recovery",
    ),
];

/// One named acceptance scenario. Definitions contain no backend or SQL terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioDefinition {
    /// Stable scenario identifier.
    pub id: &'static str,
    /// Human-readable scope used by evidence reports.
    pub scope: &'static str,
}

/// Fixed provider-neutral input used by every scenario. Adapters own all
/// authority, connection, and schema setup; this value contains only the
/// canonical identities and semantic values that the scenarios exercise.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ScenarioPlan {
    pub room_id: String,
    pub member_id: String,
    pub principal_id: String,
    pub runner_id: String,
    pub action_id: String,
    pub transition_id: String,
    pub timer_id: String,
    pub activation_id: String,
    pub claim_id: String,
    pub operation_prefix: String,
    pub admitted_at: String,
    pub scheduled_for: String,
    pub lease_until: String,
    pub action_payload: Vec<u8>,
    pub timer_payload: Vec<u8>,
    pub projection_bytes: Vec<u8>,
    /// Additional fixed Heist seats used by timer/Activation scenarios. They
    /// are inputs, not adapter-owned fixtures, so both providers address the
    /// same immutable Membership and Runner targets.
    pub secondary_member_ids: Vec<String>,
    pub secondary_principal_ids: Vec<String>,
    pub activation_target_member_id: String,
    pub timer_scheduled_for: String,
}

impl ScenarioPlan {
    /// Returns the deterministic plan shared by the SQLite and PostgreSQL
    /// adapter runners.
    #[must_use]
    pub fn counter() -> Self {
        Self {
            room_id: "01ARZ3NDEKTSV4RRFFQ69G5FC5".to_owned(),
            member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC0".to_owned(),
            principal_id: "01ARZ3NDEKTSV4RRFFQ69G5FD0".to_owned(),
            runner_id: "01ARZ3NDEKTSV4RRFFQ69G5FE0".to_owned(),
            action_id: "01ARZ3NDEKTSV4RRFFQ69G5FC6".to_owned(),
            transition_id: "01ARZ3NDEKTSV4RRFFQ69G5FQ7".to_owned(),
            timer_id: "01ARZ3NDEKTSV4RRFFQ69G5FH0".to_owned(),
            activation_id: "activation-shared-v1".to_owned(),
            claim_id: "claim-shared-v1".to_owned(),
            operation_prefix: "shared-kernel-v1".to_owned(),
            admitted_at: "2026-08-15T12:00:01Z".to_owned(),
            scheduled_for: "2026-08-15T12:00:00Z".to_owned(),
            lease_until: "2026-08-15T12:01:00Z".to_owned(),
            action_payload: br"{}".to_vec(),
            timer_payload: br#"{"tick":1}"#.to_vec(),
            projection_bytes: br#"{"projection":"shared"}"#.to_vec(),
            secondary_member_ids: vec![
                "01ARZ3NDEKTSV4RRFFQ69G5FC1".to_owned(),
                "01ARZ3NDEKTSV4RRFFQ69G5FC2".to_owned(),
                "01ARZ3NDEKTSV4RRFFQ69G5FC3".to_owned(),
                "01ARZ3NDEKTSV4RRFFQ69G5FC4".to_owned(),
            ],
            secondary_principal_ids: vec![
                "01ARZ3NDEKTSV4RRFFQ69G5FD1".to_owned(),
                "01ARZ3NDEKTSV4RRFFQ69G5FD2".to_owned(),
                "01ARZ3NDEKTSV4RRFFQ69G5FD3".to_owned(),
                "01ARZ3NDEKTSV4RRFFQ69G5FD4".to_owned(),
            ],
            activation_target_member_id: "01ARZ3NDEKTSV4RRFFQ69G5FC1".to_owned(),
            timer_scheduled_for: "2026-08-15T12:00:30Z".to_owned(),
        }
    }
}

/// An exact byte-bearing observation that is compared across providers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanonicalFact {
    pub kind: String,
    pub key: String,
    pub bytes: Vec<u8>,
    pub hash: Vec<u8>,
    pub semantic: String,
}

/// Provider-neutral evidence returned by one scenario adapter operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScenarioEvidence {
    pub outcomes: Vec<CanonicalOperationOutcome>,
    pub projections: Vec<ProjectionOutcome>,
    pub facts: Vec<CanonicalFact>,
    pub notes: Vec<String>,
}

impl ScenarioEvidence {
    /// Creates a successful evidence envelope. All required scenario bytes
    /// must be supplied by the adapter; there is no incomplete constructor.
    #[must_use]
    pub fn pass(
        outcomes: Vec<CanonicalOperationOutcome>,
        projections: Vec<ProjectionOutcome>,
        facts: Vec<CanonicalFact>,
        notes: Vec<String>,
    ) -> Self {
        Self {
            outcomes,
            projections,
            facts,
            notes,
        }
    }
}

/// Closed adapter failure. The shared runner records this as a failed test,
/// never as a parity success or an incomplete result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterError {
    pub operation: String,
    pub detail: String,
}

impl AdapterError {
    /// Creates a stable, provider-neutral operation failure.
    #[must_use]
    pub fn new(operation: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            operation: operation.into(),
            detail: detail.into(),
        }
    }
}

/// The real black-box adapter contract. Backend setup belongs to each
/// implementation; the scenario definitions below are shared and unchanged.
pub trait KernelConformanceAdapter {
    /// Adapter label used only in the provider-specific artifact.
    fn adapter_name(&self) -> &'static str;
    /// Runs the create/idempotency scenario.
    fn create(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError>;
    /// Runs Core administration and receipt behavior.
    fn core_admin(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError>;
    /// Runs Participant Action behavior.
    fn action(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError>;
    /// Runs conditional Timer generation behavior.
    fn timer(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError>;
    /// Runs frame, Cursor, prune, reset, and privacy behavior.
    fn frame_cursor_reset(&mut self, plan: &ScenarioPlan)
    -> Result<ScenarioEvidence, AdapterError>;
    /// Runs Activation offer/claim/lease/tombstone behavior.
    fn activation(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError>;
    /// Runs restart, replay, snapshot, cache, and integrity behavior.
    fn recovery(&mut self, plan: &ScenarioPlan) -> Result<ScenarioEvidence, AdapterError>;
}

/// Executes the frozen scenario definitions through one adapter.
///
/// # Errors
///
/// Returns the first adapter operation failure. A failed operation is not
/// serialized as an `incomplete` result.
pub fn run_catalog<A: KernelConformanceAdapter>(
    adapter: &mut A,
    plan: &ScenarioPlan,
) -> Result<Vec<ScenarioResult>, AdapterError> {
    let mut results = Vec::with_capacity(SCENARIOS.len());
    for definition in SCENARIOS {
        let evidence = match definition.id {
            "create" => adapter.create(plan),
            "core_admin" => adapter.core_admin(plan),
            "action" => adapter.action(plan),
            "timer" => adapter.timer(plan),
            "frame_cursor_reset" => adapter.frame_cursor_reset(plan),
            "activation" => adapter.activation(plan),
            "recovery" => adapter.recovery(plan),
            _ => Err(AdapterError::new(definition.id, "unknown scenario")),
        }?;
        if evidence.outcomes.is_empty()
            || evidence.projections.is_empty()
            || evidence.facts.is_empty()
        {
            return Err(AdapterError::new(
                definition.id,
                "adapter returned an empty required evidence section",
            ));
        }
        if evidence.projections.iter().any(|projection| {
            projection.status != ScenarioStatus::Pass
                || projection.projection_bytes.is_none()
                || projection.projection_hash.is_none()
                || projection.projection_hash.as_deref()
                    != projection
                        .projection_bytes
                        .as_deref()
                        .map(digest)
                        .as_deref()
        }) {
            return Err(AdapterError::new(
                definition.id,
                "adapter returned a non-pass or byte-less projection",
            ));
        }
        if evidence.facts.iter().any(|fact| {
            fact.kind.is_empty()
                || fact.key.is_empty()
                || fact.semantic.is_empty()
                || fact.hash != digest(&fact.bytes)
        }) {
            return Err(AdapterError::new(
                definition.id,
                "adapter returned an invalid canonical fact hash or identity",
            ));
        }
        results.push(ScenarioResult {
            adapter: adapter.adapter_name().to_owned(),
            scenario: definition.id.to_owned(),
            status: ScenarioStatus::Pass,
            outcomes: evidence.outcomes,
            projections: evidence.projections,
            facts: evidence.facts,
            notes: evidence.notes,
        });
    }
    Ok(results)
}

/// Executes the exact same scenario definitions through two adapters and
/// compares the complete provider-neutral artifact. Backend provisioning is
/// deliberately outside this function; both adapters receive the same plan.
///
/// # Errors
///
/// Returns an adapter failure or an exact canonical artifact mismatch.
pub fn run_and_compare<A: KernelConformanceAdapter, B: KernelConformanceAdapter>(
    left: &mut A,
    right: &mut B,
    plan: &ScenarioPlan,
) -> Result<(Vec<ScenarioResult>, Vec<ScenarioResult>), AdapterError> {
    let left_results = run_catalog(left, plan)?;
    let right_results = run_catalog(right, plan)?;
    ConformanceArtifact::compare_adapters(&left_results, &right_results)
        .map_err(|detail| AdapterError::new("cross-adapter", detail))?;
    Ok((left_results, right_results))
}

impl ScenarioDefinition {
    const fn new(id: &'static str, scope: &'static str) -> Self {
        Self { id, scope }
    }
}

/// Status of one adapter's execution of one shared scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    Pass,
    Incomplete,
    Failed,
}

/// A canonical black-box operation observation. Bytes are represented as
/// arrays rather than hex strings so the canonical transcript preserves the
/// exact byte values and does not depend on a provider codec.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CanonicalOperationOutcome {
    pub operation: String,
    pub resolution: String,
    pub duplicate: bool,
    pub receipt_bytes: Option<Vec<u8>>,
    pub receipt_hash: Option<Vec<u8>>,
    pub resolved: String,
    pub resolved_receipt_bytes: Option<Vec<u8>>,
    pub resolved_receipt_hash: Option<Vec<u8>>,
}

/// A viewer projection comparison row. The full suite reserves this field so
/// viewer bytes cannot be omitted when a provider claims a scenario pass.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectionOutcome {
    pub operation: String,
    pub status: ScenarioStatus,
    pub projection_bytes: Option<Vec<u8>>,
    pub projection_hash: Option<Vec<u8>>,
}

/// Result for one adapter and one shared scenario.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub adapter: String,
    pub scenario: String,
    pub status: ScenarioStatus,
    pub outcomes: Vec<CanonicalOperationOutcome>,
    pub projections: Vec<ProjectionOutcome>,
    pub facts: Vec<CanonicalFact>,
    pub notes: Vec<String>,
}

/// Complete canonical result artifact. `canonical_bytes` and `canonical_hash`
/// are computed from the exact JSON payload in `results`; consumers compare
/// these fields before interpreting semantic labels.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ConformanceArtifact {
    pub schema: String,
    pub scenarios: Vec<ScenarioDefinition>,
    pub results: Vec<ScenarioResult>,
    pub canonical_bytes: Vec<u8>,
    pub canonical_hash: Vec<u8>,
}

impl ConformanceArtifact {
    /// Builds the canonical artifact from adapter results.
    ///
    /// # Errors
    ///
    /// Returns the JSON serialization or canonicalization error if the
    /// result schema contains a value that cannot be encoded.
    pub fn new(results: Vec<ScenarioResult>) -> Result<Self, serde_json::Error> {
        let payload = ArtifactPayload {
            schema: "worldstream/kernel-conformance/v1".to_owned(),
            scenarios: SCENARIOS.to_vec(),
            results: results.clone(),
        };
        let json = serde_json::to_vec(&payload)?;
        let canonical = CanonicalJsonV1::parse(&json)
            .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))?;
        let bytes = canonical
            .to_bytes()
            .map_err(|error| serde_json::Error::io(std::io::Error::other(error.to_string())))?;
        let hash = blake3::hash(&bytes).as_bytes().to_vec();
        Ok(Self {
            schema: payload.schema,
            scenarios: payload.scenarios,
            results,
            canonical_bytes: bytes,
            canonical_hash: hash,
        })
    }

    /// Builds the provider-neutral comparison artifact. Provider labels are
    /// deliberately removed from this transcript so canonical bytes and the
    /// resulting hash describe semantic behavior rather than adapter names.
    ///
    /// # Errors
    ///
    /// Returns the same serialization or canonicalization errors as [`Self::new`].
    pub fn for_comparison(results: &[ScenarioResult]) -> Result<Self, serde_json::Error> {
        let mut normalized = results.to_vec();
        for result in &mut normalized {
            "adapter".clone_into(&mut result.adapter);
        }
        Self::new(normalized)
    }

    /// Compares two adapter runs after removing only provider labels. Exact
    /// canonical bytes, hashes, semantic outcome labels, projection bytes,
    /// and facts all remain part of the comparison payload.
    pub fn compare_adapters(
        left: &[ScenarioResult],
        right: &[ScenarioResult],
    ) -> Result<(), String> {
        let left = Self::for_comparison(left).map_err(|error| error.to_string())?;
        let right = Self::for_comparison(right).map_err(|error| error.to_string())?;
        if left.canonical_bytes == right.canonical_bytes
            && left.canonical_hash == right.canonical_hash
        {
            Ok(())
        } else {
            Err(format!(
                "provider-neutral canonical artifacts differ: left={} right={}",
                encode_hex(&left.canonical_hash),
                encode_hex(&right.canonical_hash),
            ))
        }
    }
}

#[derive(Serialize)]
struct ArtifactPayload {
    schema: String,
    scenarios: Vec<ScenarioDefinition>,
    results: Vec<ScenarioResult>,
}

/// Canonical BLAKE3 bytes for an exact receipt, kept independent from the
/// adapter's display or database representation.
#[must_use]
pub fn digest(bytes: &[u8]) -> Vec<u8> {
    blake3::hash(bytes).as_bytes().to_vec()
}

/// Builds one exact operation outcome from provider-neutral labels and bytes.
#[must_use]
pub fn operation_outcome(
    operation: impl Into<String>,
    resolution: impl Into<String>,
    duplicate: bool,
    receipt_bytes: Option<Vec<u8>>,
    resolved: impl Into<String>,
    resolved_receipt_bytes: Option<Vec<u8>>,
) -> CanonicalOperationOutcome {
    let receipt_hash = receipt_bytes.as_deref().map(digest);
    let resolved_receipt_hash = resolved_receipt_bytes.as_deref().map(digest);
    CanonicalOperationOutcome {
        operation: operation.into(),
        resolution: resolution.into(),
        duplicate,
        receipt_bytes,
        receipt_hash,
        resolved: resolved.into(),
        resolved_receipt_bytes,
        resolved_receipt_hash,
    }
}

/// Compares exact operation outcomes, including receipt bytes and hashes.
#[must_use]
pub fn compare_exact(left: &CanonicalOperationOutcome, right: &CanonicalOperationOutcome) -> bool {
    left == right
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Exact upstream A202 source revision pinned by ADR 0015.
pub const A202_PINNED_REVISION: &str = "fa85aa8b49bfe7b3f7ded487c98500a600e92e41";

/// The only four Negotiate Roles.
pub const ROLES: [Role; 4] = [
    Role::BuyerAgent,
    Role::SellerAgent,
    Role::BuyerApprover,
    Role::VenueSigner,
];

/// The only eight Activity Phases.
pub const PHASES: [Phase; 8] = [
    Phase::FormationOpen,
    Phase::ApprovalPending,
    Phase::OfferAccepted,
    Phase::AgreementPending,
    Phase::WithdrawnPendingExpiry,
    Phase::DeadlineResolution,
    Phase::Complete,
    Phase::Expired,
];

/// The only eleven participant Action types.
pub const ACTIONS: [ActionKind; 11] = [
    ActionKind::SubmitProposalRevision,
    ActionKind::WithdrawLiveProposal,
    ActionKind::RequestExactApproval,
    ActionKind::RecordExactApproval,
    ActionKind::AcceptCurrentProposal,
    ActionKind::SelectAcceptedProposal,
    ActionKind::RecordAgreementSignature,
    ActionKind::CommitAgreement,
    ActionKind::RecordSessionDeadlineElapsed,
    ActionKind::RecordTransactionDeadlineElapsed,
    ActionKind::CloseTransactionExpiredSession,
];

/// The only four Agent Attention reasons.
pub const ATTENTION_REASONS: [AttentionReason; 4] = [
    AttentionReason::ProposalReceived,
    AttentionReason::AgreementSignatureRequired,
    AttentionReason::AgreementReady,
    AttentionReason::VenueSignatureRequired,
];

/// The six privacy-test personas. Spectator and Operator are access modes,
/// never Negotiate Roles.
pub const PERSONAS: [Persona; 6] = [
    Persona::BuyerAgent,
    Persona::SellerAgent,
    Persona::BuyerApprover,
    Persona::VenueSigner,
    Persona::Operator,
    Persona::Spectator,
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    BuyerAgent,
    SellerAgent,
    BuyerApprover,
    VenueSigner,
}

impl Role {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BuyerAgent => "buyer_agent",
            Self::SellerAgent => "seller_agent",
            Self::BuyerApprover => "buyer_approver",
            Self::VenueSigner => "venue_signer",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Persona {
    BuyerAgent,
    SellerAgent,
    BuyerApprover,
    VenueSigner,
    Operator,
    Spectator,
}

impl Persona {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BuyerAgent => "buyer_agent",
            Self::SellerAgent => "seller_agent",
            Self::BuyerApprover => "buyer_approver",
            Self::VenueSigner => "venue_signer",
            Self::Operator => "operator",
            Self::Spectator => "spectator",
        }
    }
}

impl From<Role> for Persona {
    fn from(value: Role) -> Self {
        match value {
            Role::BuyerAgent => Self::BuyerAgent,
            Role::SellerAgent => Self::SellerAgent,
            Role::BuyerApprover => Self::BuyerApprover,
            Role::VenueSigner => Self::VenueSigner,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    FormationOpen,
    ApprovalPending,
    OfferAccepted,
    AgreementPending,
    WithdrawnPendingExpiry,
    DeadlineResolution,
    Complete,
    Expired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    SubmitProposalRevision,
    WithdrawLiveProposal,
    RequestExactApproval,
    RecordExactApproval,
    AcceptCurrentProposal,
    SelectAcceptedProposal,
    RecordAgreementSignature,
    CommitAgreement,
    RecordSessionDeadlineElapsed,
    RecordTransactionDeadlineElapsed,
    CloseTransactionExpiredSession,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionReason {
    ProposalReceived,
    AgreementSignatureRequired,
    AgreementReady,
    VenueSignatureRequired,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerKind {
    ProposalValidity,
    ApprovalExpiry,
    FormationDeadline,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyClass {
    ProposalRevision,
    CurrentTermDiff,
    BuyerAcceptanceEnvelope,
    SignedApproval,
    SignedStreamEvent,
    CommittedAgreement,
    OperationalDiagnostic,
    PublicStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyExposure {
    None,
    Reference,
    Bounded,
    Full,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RoomHead {
    pub sequence: u64,
    pub digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LogicalHead {
    pub sequence: u64,
    pub event_hash: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ActionBasis {
    pub room: RoomHead,
    pub transaction: LogicalHead,
    pub session: LogicalHead,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FixtureSignature {
    pub signer_id: String,
    pub signer_role: Role,
    pub purpose: String,
    pub signed_wire_digest: String,
    pub proof: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExactA202Object {
    pub object_id: String,
    pub object_type: String,
    pub declared_content_hash: String,
    /// Exact UTF-8 canonical JSON. This string is serialized as a string so a
    /// consumer can recover byte-for-byte input with `as_bytes()`.
    pub canonical_json: String,
    pub wire_digest: String,
    pub signature: FixtureSignature,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Proposal {
    pub author: Role,
    pub offer: ExactA202Object,
    pub session_event: ExactA202Object,
    pub valid_until: u64,
    pub supersedes_offer_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ApprovalBinding {
    pub candidate_canonical_json: String,
    pub candidate_wire_digest: String,
    pub transaction_id: String,
    pub proposal_id: String,
    pub proposal_content_hash: String,
    pub room_head_at_request: RoomHead,
    pub transaction_head_at_request: LogicalHead,
    pub session_head_at_request: LogicalHead,
    pub approver_id: String,
    pub expires_at: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExactApproval {
    pub binding: ApprovalBinding,
    pub decision: ApprovalDecision,
    pub approval: ExactA202Object,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Approved,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgreementSignature {
    pub agreement_id: String,
    pub agreement_content_hash: String,
    pub agreement_canonical_json: String,
    pub agreement_wire_digest: String,
    pub signer_id: String,
    pub signer_role: Role,
    pub proof: String,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeadlineProgress {
    #[default]
    None,
    AwaitingSessionExpiry,
    AwaitingTransactionExpiry,
    AwaitingSessionClose,
    Resolved,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Outcome {
    AgreementCommitted {
        transaction_id: String,
        agreement_id: String,
        agreement_hash: String,
    },
    FormationExpired {
        transaction_id: String,
        final_session_state: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EvidenceLink {
    pub object_id: String,
    pub object_type: String,
    pub content_hash: String,
    pub wire_digest: String,
    pub action_id: String,
    pub room_sequence: u64,
    pub room_digest: String,
    pub transaction_head: LogicalHead,
    pub session_head: LogicalHead,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OracleState {
    pub oracle_id: String,
    pub a202_revision: String,
    pub transaction_id: String,
    pub session_id: String,
    pub phase: Phase,
    pub aggregate_state: String,
    pub session_state: String,
    pub room_head: RoomHead,
    pub transaction_head: LogicalHead,
    pub session_head: LogicalHead,
    pub formation_deadline: u64,
    pub proposal_expired: bool,
    pub current_proposal: Option<Proposal>,
    pub pending_approval: Option<ApprovalBinding>,
    pub recorded_approval: Option<ExactApproval>,
    pub accepted_offer_id: Option<String>,
    pub accepted_offer_hash: Option<String>,
    pub agreement_id: Option<String>,
    pub agreement_content_hash: Option<String>,
    pub agreement_canonical_json: Option<String>,
    pub agreement_signatures: BTreeMap<Role, AgreementSignature>,
    pub deadline_progress: DeadlineProgress,
    pub outcome: Option<Outcome>,
    pub evidence: Vec<EvidenceLink>,
}

impl OracleState {
    #[must_use]
    pub fn basis(&self) -> ActionBasis {
        ActionBasis {
            room: self.room_head.clone(),
            transaction: self.transaction_head.clone(),
            session: self.session_head.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TimerFired {
    pub timer: TimerKind,
    pub generation: u64,
    pub scheduled_for: u64,
    pub fired_at: u64,
    pub expected_object_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "stimulus", content = "value", rename_all = "snake_case")]
pub enum Stimulus {
    Action(Action),
    TimerFired(TimerFired),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    SubmitProposalRevision {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        proposal: Proposal,
    },
    WithdrawLiveProposal {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        withdrawal_event: ExactA202Object,
    },
    RequestExactApproval {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        binding: ApprovalBinding,
    },
    RecordExactApproval {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        approval: ExactApproval,
    },
    AcceptCurrentProposal {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        candidate_canonical_json: String,
        approval: ExactApproval,
        acceptance: ExactA202Object,
        acceptance_event: ExactA202Object,
    },
    SelectAcceptedProposal {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        selection_event: ExactA202Object,
    },
    RecordAgreementSignature {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        signature: AgreementSignature,
    },
    CommitAgreement {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        agreement: ExactA202Object,
        commitment_event: ExactA202Object,
    },
    RecordSessionDeadlineElapsed {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        occurred_at: u64,
        deadline_event: ExactA202Object,
    },
    RecordTransactionDeadlineElapsed {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        occurred_at: u64,
        deadline_event: ExactA202Object,
    },
    CloseTransactionExpiredSession {
        action_id: String,
        actor: Role,
        basis: ActionBasis,
        admitted_at: u64,
        close_event: ExactA202Object,
    },
}

impl Action {
    #[must_use]
    pub const fn kind(&self) -> ActionKind {
        match self {
            Self::SubmitProposalRevision { .. } => ActionKind::SubmitProposalRevision,
            Self::WithdrawLiveProposal { .. } => ActionKind::WithdrawLiveProposal,
            Self::RequestExactApproval { .. } => ActionKind::RequestExactApproval,
            Self::RecordExactApproval { .. } => ActionKind::RecordExactApproval,
            Self::AcceptCurrentProposal { .. } => ActionKind::AcceptCurrentProposal,
            Self::SelectAcceptedProposal { .. } => ActionKind::SelectAcceptedProposal,
            Self::RecordAgreementSignature { .. } => ActionKind::RecordAgreementSignature,
            Self::CommitAgreement { .. } => ActionKind::CommitAgreement,
            Self::RecordSessionDeadlineElapsed { .. } => ActionKind::RecordSessionDeadlineElapsed,
            Self::RecordTransactionDeadlineElapsed { .. } => {
                ActionKind::RecordTransactionDeadlineElapsed
            }
            Self::CloseTransactionExpiredSession { .. } => {
                ActionKind::CloseTransactionExpiredSession
            }
        }
    }

    #[must_use]
    pub fn action_id(&self) -> &str {
        match self {
            Self::SubmitProposalRevision { action_id, .. }
            | Self::WithdrawLiveProposal { action_id, .. }
            | Self::RequestExactApproval { action_id, .. }
            | Self::RecordExactApproval { action_id, .. }
            | Self::AcceptCurrentProposal { action_id, .. }
            | Self::SelectAcceptedProposal { action_id, .. }
            | Self::RecordAgreementSignature { action_id, .. }
            | Self::CommitAgreement { action_id, .. }
            | Self::RecordSessionDeadlineElapsed { action_id, .. }
            | Self::RecordTransactionDeadlineElapsed { action_id, .. }
            | Self::CloseTransactionExpiredSession { action_id, .. } => action_id,
        }
    }

    #[must_use]
    pub const fn actor(&self) -> Role {
        match self {
            Self::SubmitProposalRevision { actor, .. }
            | Self::WithdrawLiveProposal { actor, .. }
            | Self::RequestExactApproval { actor, .. }
            | Self::RecordExactApproval { actor, .. }
            | Self::AcceptCurrentProposal { actor, .. }
            | Self::SelectAcceptedProposal { actor, .. }
            | Self::RecordAgreementSignature { actor, .. }
            | Self::CommitAgreement { actor, .. }
            | Self::RecordSessionDeadlineElapsed { actor, .. }
            | Self::RecordTransactionDeadlineElapsed { actor, .. }
            | Self::CloseTransactionExpiredSession { actor, .. } => *actor,
        }
    }

    #[must_use]
    pub const fn admitted_at(&self) -> u64 {
        match self {
            Self::SubmitProposalRevision { admitted_at, .. }
            | Self::WithdrawLiveProposal { admitted_at, .. }
            | Self::RequestExactApproval { admitted_at, .. }
            | Self::RecordExactApproval { admitted_at, .. }
            | Self::AcceptCurrentProposal { admitted_at, .. }
            | Self::SelectAcceptedProposal { admitted_at, .. }
            | Self::RecordAgreementSignature { admitted_at, .. }
            | Self::CommitAgreement { admitted_at, .. }
            | Self::RecordSessionDeadlineElapsed { admitted_at, .. }
            | Self::RecordTransactionDeadlineElapsed { admitted_at, .. }
            | Self::CloseTransactionExpiredSession { admitted_at, .. } => *admitted_at,
        }
    }

    #[must_use]
    pub const fn basis(&self) -> &ActionBasis {
        match self {
            Self::SubmitProposalRevision { basis, .. }
            | Self::WithdrawLiveProposal { basis, .. }
            | Self::RequestExactApproval { basis, .. }
            | Self::RecordExactApproval { basis, .. }
            | Self::AcceptCurrentProposal { basis, .. }
            | Self::SelectAcceptedProposal { basis, .. }
            | Self::RecordAgreementSignature { basis, .. }
            | Self::CommitAgreement { basis, .. }
            | Self::RecordSessionDeadlineElapsed { basis, .. }
            | Self::RecordTransactionDeadlineElapsed { basis, .. }
            | Self::CloseTransactionExpiredSession { basis, .. } => basis,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Attention {
    pub target: Role,
    pub reason: AttentionReason,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Transition {
    pub action_id: String,
    pub action_kind: Option<ActionKind>,
    pub previous_room_head: RoomHead,
    pub state: OracleState,
    pub attention: Vec<Attention>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RejectionCode {
    StaleRoomHead,
    StaleTransactionHead,
    StaleSessionHead,
    InvalidPhase,
    RoleViolation,
    WrongSigner,
    InvalidSignature,
    NonCanonicalA202Bytes,
    ByteMutation,
    ApprovalBindingMismatch,
    ApprovalExpired,
    ProposalExpired,
    DeadlineReached,
    DeadlineNotPassed,
    ResourceLimit,
    ForbiddenImport,
    CallbackFaultBeforeCommit,
    MissingRetainedBundle,
    CorruptRetainedBundle,
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
#[error("{code:?}: {detail}")]
pub struct OracleError {
    pub code: RejectionCode,
    pub detail: String,
}

impl OracleError {
    #[must_use]
    pub fn new(code: RejectionCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleCondition {
    Available,
    Missing,
    Corrupt,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BoundaryProbe {
    pub imports: Vec<String>,
    pub component_bytes: u64,
    pub callback_fault_before_commit: bool,
    pub retained_bundle: BundleCondition,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoldenCheckpoint {
    pub label: String,
    pub phase: Phase,
    pub room_head: RoomHead,
    pub transaction_head: LogicalHead,
    pub session_head: LogicalHead,
    pub outcome: Option<Outcome>,
    pub state_digest: String,
}

/// Serialize into the independent canonical-JSON subset used by this oracle.
/// Object keys are sorted, floats and integers outside JavaScript's exact
/// range are rejected, and no whitespace is emitted.
pub fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, OracleError> {
    let value = serde_json::to_value(value).map_err(|error| {
        OracleError::new(
            RejectionCode::NonCanonicalA202Bytes,
            format!("cannot encode canonical JSON: {error}"),
        )
    })?;
    let mut output = String::new();
    write_canonical(&value, &mut output)?;
    Ok(output.into_bytes())
}

fn write_canonical(value: &serde_json::Value, output: &mut String) -> Result<(), OracleError> {
    match value {
        serde_json::Value::Null => output.push_str("null"),
        serde_json::Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        serde_json::Value::Number(number) => {
            const MAX_SAFE: u64 = 9_007_199_254_740_991;
            let safe = number.as_u64().is_some_and(|value| value <= MAX_SAFE)
                || number
                    .as_i64()
                    .is_some_and(|value| value.unsigned_abs() <= MAX_SAFE);
            if !safe {
                return Err(OracleError::new(
                    RejectionCode::NonCanonicalA202Bytes,
                    "floating-point or unsafe JSON number",
                ));
            }
            output.push_str(&number.to_string());
        }
        serde_json::Value::String(value) => {
            let encoded = serde_json::to_string(value).map_err(|error| {
                OracleError::new(
                    RejectionCode::NonCanonicalA202Bytes,
                    format!("cannot encode JSON string: {error}"),
                )
            })?;
            output.push_str(&encoded);
        }
        serde_json::Value::Array(values) => {
            output.push('[');
            for (index, item) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical(item, output)?;
            }
            output.push(']');
        }
        serde_json::Value::Object(values) => {
            output.push('{');
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_by_key(|(key, _)| *key);
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let encoded = serde_json::to_string(key).map_err(|error| {
                    OracleError::new(
                        RejectionCode::NonCanonicalA202Bytes,
                        format!("cannot encode JSON key: {error}"),
                    )
                })?;
                output.push_str(&encoded);
                output.push(':');
                write_canonical(item, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

#[must_use]
pub fn tagged_blake3(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

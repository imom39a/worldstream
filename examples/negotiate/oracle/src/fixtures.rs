use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::oracle::{agreement_signature, fixture_signature};
use crate::{
    A202_PINNED_REVISION, ACTIONS, Action, ApprovalBinding, ApprovalDecision, BoundaryProbe,
    BundleCondition, EvidenceLink, ExactA202Object, ExactApproval, GoldenCheckpoint, Oracle,
    OracleError, OracleState, Outcome, PERSONAS, PHASES, PRIVACY_MATRIX, Phase, PrivacyMatrixRow,
    ROLES, RejectionCode, Role, Stimulus, TimerFired, TimerKind, canonical_bytes, tagged_blake3,
    validate_boundary_probe,
};

const FORMATION_DEADLINE: u64 = 1_000;

/// Checked-in, human-readable mirror of [`corpus_bytes`]. Consumers in other
/// languages can use this fixture without linking Rust.
pub const CORPUS_FIXTURE_BYTES: &[u8] = include_bytes!("../fixtures/corpus-v1.json");

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoldenPlanV1 {
    pub plan_id: String,
    pub formation_deadline: u64,
    pub restart_after_step: usize,
    pub stimuli: Vec<Stimulus>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoldenStepV1 {
    pub index: usize,
    pub label: String,
    pub stimulus: Stimulus,
    pub expected: GoldenCheckpoint,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GoldenCorpusV1 {
    pub corpus_id: String,
    pub oracle_id: String,
    pub a202_revision: String,
    pub a202_fixture_provenance: String,
    pub restart_after_step: usize,
    pub steps: Vec<GoldenStepV1>,
    pub final_state: OracleState,
    pub final_state_digest: String,
    pub outcome: Outcome,
    pub evidence_cross_index: Vec<EvidenceLink>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NegativeKindV1 {
    MutatedApprovedByte,
    StaleRoomHead,
    ChangedTransactionHead,
    ChangedSessionHead,
    ExpiredProposalValidity,
    ExpiredApproval,
    WrongSigner,
    RoleAuthorityViolation,
    DeadlineAtEquality,
    SignedFormationExpiry,
    PrivacyNoninterference,
    ForbiddenImport,
    ResourceExhaustion,
    CallbackFaultBeforeCommit,
    MissingRetainedBundle,
    CorruptRetainedBundle,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NegativeCaseV1 {
    pub id: String,
    pub kind: NegativeKindV1,
    pub expected_code: Option<RejectionCode>,
    pub invariant: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PrivacyMutationCaseV1 {
    pub source: crate::Persona,
    pub viewer: crate::Persona,
    pub expected_projection_unchanged: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AlternateOutcomeV1 {
    pub outcome: Outcome,
    pub final_state_digest: String,
    pub evidence_cross_index: Vec<EvidenceLink>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CorpusV1 {
    pub corpus_id: String,
    pub roles: [Role; 4],
    pub phases: [Phase; 8],
    pub actions: [crate::ActionKind; 11],
    pub attention_reasons: [crate::AttentionReason; 4],
    pub personas: [crate::Persona; 6],
    pub privacy_matrix: [PrivacyMatrixRow; 8],
    pub privacy_mutations: Vec<PrivacyMutationCaseV1>,
    pub golden: GoldenCorpusV1,
    pub signed_expiry: AlternateOutcomeV1,
    pub negatives: Vec<NegativeCaseV1>,
}

pub fn corpus() -> Result<CorpusV1, OracleError> {
    let expiry_state = run_expiry_path()?;
    let expiry_outcome = expiry_state.outcome.clone().ok_or_else(|| {
        OracleError::new(
            RejectionCode::InvalidPhase,
            "signed expiry fixture did not establish an Outcome",
        )
    })?;
    Ok(CorpusV1 {
        corpus_id: "worldstream/negotiate-conformance-corpus/v1".to_owned(),
        roles: ROLES,
        phases: PHASES,
        actions: ACTIONS,
        attention_reasons: crate::ATTENTION_REASONS,
        personas: PERSONAS,
        privacy_matrix: PRIVACY_MATRIX,
        privacy_mutations: privacy_mutation_cases(),
        golden: golden_corpus()?,
        signed_expiry: AlternateOutcomeV1 {
            outcome: expiry_outcome,
            final_state_digest: tagged_blake3(&canonical_bytes(&expiry_state)?),
            evidence_cross_index: expiry_state.evidence,
        },
        negatives: negative_cases(),
    })
}

#[must_use]
pub fn privacy_mutation_cases() -> Vec<PrivacyMutationCaseV1> {
    PERSONAS
        .into_iter()
        .flat_map(|source| {
            PERSONAS.into_iter().filter_map(move |viewer| {
                (source != viewer).then_some(PrivacyMutationCaseV1 {
                    source,
                    viewer,
                    expected_projection_unchanged: true,
                })
            })
        })
        .collect()
}

pub fn corpus_bytes() -> Result<Vec<u8>, OracleError> {
    canonical_bytes(&corpus()?)
}

pub fn golden_plan() -> Result<GoldenPlanV1, OracleError> {
    let oracle = Oracle;
    let mut state = Oracle::initialize(FORMATION_DEADLINE);
    let mut stimuli = Vec::new();

    let buyer_offer = proposal(
        &state,
        Role::BuyerAgent,
        "off_worldstream_buyer_01",
        None,
        900,
        "3350.00",
    )?;
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::SubmitProposalRevision {
            action_id: "act_01_buyer_proposal".to_owned(),
            actor: Role::BuyerAgent,
            basis,
            admitted_at: 100,
            proposal: buyer_offer,
        },
    )?;

    let seller_counter = proposal(
        &state,
        Role::SellerAgent,
        "off_worldstream_seller_02",
        Some("off_worldstream_buyer_01"),
        850,
        "3200.00",
    )?;
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::SubmitProposalRevision {
            action_id: "act_02_seller_counter".to_owned(),
            actor: Role::SellerAgent,
            basis,
            admitted_at: 200,
            proposal: seller_counter,
        },
    )?;

    let binding = approval_binding(&state, 500)?;
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::RequestExactApproval {
            action_id: "act_03_request_exact_approval".to_owned(),
            actor: Role::BuyerAgent,
            basis,
            admitted_at: 240,
            binding: binding.clone(),
        },
    )?;

    let approval = exact_approval(&binding, ApprovalDecision::Approved)?;
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::RecordExactApproval {
            action_id: "act_04_human_signed_approval".to_owned(),
            actor: Role::BuyerApprover,
            basis,
            admitted_at: 260,
            approval: approval.clone(),
        },
    )?;

    let acceptance = object(
        "acc_worldstream_01",
        "acceptance",
        Role::BuyerAgent,
        "offer_acceptance",
        json!({
            "accepting_party": "org_northstar",
            "offer_hash": binding.proposal_content_hash,
            "offer_id": binding.proposal_id,
            "session_id": state.session_id,
        }),
    )?;
    let acceptance_event = stream_event(
        &state,
        "evt_session_accept_03",
        "session",
        "offer.accepted",
        Role::VenueSigner,
    )?;
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::AcceptCurrentProposal {
            action_id: "act_05_accept_current_proposal".to_owned(),
            actor: Role::BuyerAgent,
            basis,
            admitted_at: 300,
            candidate_canonical_json: binding.candidate_canonical_json.clone(),
            approval,
            acceptance,
            acceptance_event,
        },
    )?;

    let selection_event = stream_event(
        &state,
        "evt_transaction_select_04",
        "transaction",
        "offer.selected",
        Role::BuyerAgent,
    )?;
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::SelectAcceptedProposal {
            action_id: "act_06_select_accepted_proposal".to_owned(),
            actor: Role::BuyerAgent,
            basis,
            admitted_at: 320,
            selection_event,
        },
    )?;

    let agreement_json = agreement_json(&state)?;
    let agreement_hash = content_hash("agr_worldstream_01");
    let buyer_signature = agreement_signature(
        Role::BuyerAgent,
        "agr_worldstream_01",
        &agreement_hash,
        &agreement_json,
    );
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::RecordAgreementSignature {
            action_id: "act_07_buyer_agreement_signature".to_owned(),
            actor: Role::BuyerAgent,
            basis,
            admitted_at: 340,
            signature: buyer_signature,
        },
    )?;

    let seller_signature = agreement_signature(
        Role::SellerAgent,
        "agr_worldstream_01",
        &agreement_hash,
        &agreement_json,
    );
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::RecordAgreementSignature {
            action_id: "act_08_seller_agreement_signature".to_owned(),
            actor: Role::SellerAgent,
            basis,
            admitted_at: 360,
            signature: seller_signature,
        },
    )?;

    let agreement = exact_object_from_canonical(
        "agr_worldstream_01",
        "agreement",
        agreement_hash,
        agreement_json,
        Role::BuyerAgent,
        "agreement_commitment",
    );
    let commitment_event = stream_event(
        &state,
        "evt_transaction_commit_05",
        "transaction",
        "agreement.committed",
        Role::BuyerAgent,
    )?;
    let basis = state.basis();
    apply_and_push(
        &oracle,
        &mut state,
        &mut stimuli,
        Action::CommitAgreement {
            action_id: "act_09_dual_signed_commitment".to_owned(),
            actor: Role::BuyerAgent,
            basis,
            admitted_at: 380,
            agreement,
            commitment_event,
        },
    )?;

    Ok(GoldenPlanV1 {
        plan_id: "worldstream/negotiate-golden-plan/v1".to_owned(),
        formation_deadline: FORMATION_DEADLINE,
        restart_after_step: 4,
        stimuli,
    })
}

pub fn golden_corpus() -> Result<GoldenCorpusV1, OracleError> {
    let plan = golden_plan()?;
    let oracle = Oracle;
    let mut state = Oracle::initialize(plan.formation_deadline);
    let mut steps = Vec::new();
    for (offset, stimulus) in plan.stimuli.iter().enumerate() {
        let transition = oracle.apply(&state, stimulus)?;
        state = transition.state;
        let state_bytes = canonical_bytes(&state)?;
        steps.push(GoldenStepV1 {
            index: offset + 1,
            label: transition.action_id,
            stimulus: stimulus.clone(),
            expected: GoldenCheckpoint {
                label: format!("after_step_{}", offset + 1),
                phase: state.phase,
                room_head: state.room_head.clone(),
                transaction_head: state.transaction_head.clone(),
                session_head: state.session_head.clone(),
                outcome: state.outcome.clone(),
                state_digest: tagged_blake3(&state_bytes),
            },
        });
    }
    let outcome = state.outcome.clone().ok_or_else(|| {
        OracleError::new(
            RejectionCode::InvalidPhase,
            "golden transcript did not establish an Outcome",
        )
    })?;
    let final_state_digest = tagged_blake3(&canonical_bytes(&state)?);
    Ok(GoldenCorpusV1 {
        corpus_id: "worldstream/negotiate-golden-corpus/v1".to_owned(),
        oracle_id: state.oracle_id.clone(),
        a202_revision: A202_PINNED_REVISION.to_owned(),
        a202_fixture_provenance: concat!(
            "A202 calibration-service shapes pinned at ",
            "fa85aa8b49bfe7b3f7ded487c98500a600e92e41; ",
            "WorldStream fixture signatures are deterministic conformance proofs, not ",
            "claims of upstream cryptographic conformance"
        )
        .to_owned(),
        restart_after_step: plan.restart_after_step,
        steps,
        evidence_cross_index: state.evidence.clone(),
        final_state: state,
        final_state_digest,
        outcome,
    })
}

#[must_use]
pub fn negative_cases() -> Vec<NegativeCaseV1> {
    use NegativeKindV1::{
        CallbackFaultBeforeCommit, ChangedSessionHead, ChangedTransactionHead,
        CorruptRetainedBundle, DeadlineAtEquality, ExpiredApproval, ExpiredProposalValidity,
        ForbiddenImport, MissingRetainedBundle, MutatedApprovedByte, PrivacyNoninterference,
        ResourceExhaustion, RoleAuthorityViolation, SignedFormationExpiry, StaleRoomHead,
        WrongSigner,
    };
    [
        (
            MutatedApprovedByte,
            Some(RejectionCode::ByteMutation),
            "one changed candidate byte cannot consume approval",
        ),
        (
            StaleRoomHead,
            Some(RejectionCode::StaleRoomHead),
            "Room Head is an independent concurrency witness",
        ),
        (
            ChangedTransactionHead,
            Some(RejectionCode::StaleTransactionHead),
            "approval cannot cross a changed A202 transaction head",
        ),
        (
            ChangedSessionHead,
            Some(RejectionCode::StaleSessionHead),
            "approval cannot cross a changed A202 session head",
        ),
        (
            ExpiredProposalValidity,
            Some(RejectionCode::ProposalExpired),
            "proposal validity is half-open",
        ),
        (
            ExpiredApproval,
            Some(RejectionCode::ApprovalExpired),
            "approval expiry is half-open",
        ),
        (
            WrongSigner,
            Some(RejectionCode::WrongSigner),
            "protocol purpose must be signed by the assigned identity",
        ),
        (
            RoleAuthorityViolation,
            Some(RejectionCode::RoleViolation),
            "the eleven Actions retain exact Role authority",
        ),
        (
            DeadlineAtEquality,
            Some(RejectionCode::DeadlineNotPassed),
            "A202 signed deadline event requires strict passage",
        ),
        (
            SignedFormationExpiry,
            None,
            "expiry requires independently signed session and transaction events",
        ),
        (
            PrivacyNoninterference,
            None,
            "unauthorized pairwise mutations do not change any projection",
        ),
        (
            ForbiddenImport,
            Some(RejectionCode::ForbiddenImport),
            "portable Component has zero imports",
        ),
        (
            ResourceExhaustion,
            Some(RejectionCode::ResourceLimit),
            "resource excess fails closed",
        ),
        (
            CallbackFaultBeforeCommit,
            Some(RejectionCode::CallbackFaultBeforeCommit),
            "fault produces no authoritative commit",
        ),
        (
            MissingRetainedBundle,
            Some(RejectionCode::MissingRetainedBundle),
            "replay cannot substitute a missing revision",
        ),
        (
            CorruptRetainedBundle,
            Some(RejectionCode::CorruptRetainedBundle),
            "replay rejects corrupt retained bytes",
        ),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (kind, expected_code, invariant))| NegativeCaseV1 {
        id: format!("neg_{:02}_{kind:?}", index + 1).to_lowercase(),
        kind,
        expected_code,
        invariant: invariant.to_owned(),
    })
    .collect()
}

pub fn run_negative(kind: NegativeKindV1) -> Result<(), OracleError> {
    match kind {
        NegativeKindV1::ForbiddenImport => validate_boundary_probe(&BoundaryProbe {
            imports: vec!["wasi:filesystem/types".to_owned()],
            component_bytes: 1,
            callback_fault_before_commit: false,
            retained_bundle: BundleCondition::Available,
        }),
        NegativeKindV1::ResourceExhaustion => validate_boundary_probe(&BoundaryProbe {
            imports: Vec::new(),
            component_bytes: 32 * 1024 * 1024 + 1,
            callback_fault_before_commit: false,
            retained_bundle: BundleCondition::Available,
        }),
        NegativeKindV1::CallbackFaultBeforeCommit => validate_boundary_probe(&BoundaryProbe {
            imports: Vec::new(),
            component_bytes: 1,
            callback_fault_before_commit: true,
            retained_bundle: BundleCondition::Available,
        }),
        NegativeKindV1::MissingRetainedBundle => validate_boundary_probe(&BoundaryProbe {
            imports: Vec::new(),
            component_bytes: 1,
            callback_fault_before_commit: false,
            retained_bundle: BundleCondition::Missing,
        }),
        NegativeKindV1::CorruptRetainedBundle => validate_boundary_probe(&BoundaryProbe {
            imports: Vec::new(),
            component_bytes: 1,
            callback_fault_before_commit: false,
            retained_bundle: BundleCondition::Corrupt,
        }),
        _ => run_state_negative(kind),
    }
}

fn run_state_negative(kind: NegativeKindV1) -> Result<(), OracleError> {
    let plan = golden_plan()?;
    let oracle = Oracle;
    let mut state = Oracle::initialize(plan.formation_deadline);
    for stimulus in plan.stimuli.iter().take(4) {
        state = oracle.apply(&state, stimulus)?.state;
    }
    let accept = plan.stimuli.get(4).cloned().ok_or_else(|| {
        OracleError::new(RejectionCode::InvalidPhase, "golden acceptance missing")
    })?;
    match kind {
        NegativeKindV1::MutatedApprovedByte => {
            let mut action = action_from_stimulus(accept)?;
            if let Action::AcceptCurrentProposal {
                candidate_canonical_json,
                ..
            } = &mut action
            {
                candidate_canonical_json.push(' ');
            }
            oracle.apply(&state, &Stimulus::Action(action)).map(|_| ())
        }
        NegativeKindV1::StaleRoomHead => {
            let mut action = action_from_stimulus(accept)?;
            action_basis_mut(&mut action).room.sequence = 0;
            oracle.apply(&state, &Stimulus::Action(action)).map(|_| ())
        }
        NegativeKindV1::ChangedTransactionHead => {
            state.transaction_head.sequence += 1;
            oracle.apply(&state, &accept).map(|_| ())
        }
        NegativeKindV1::ChangedSessionHead => {
            state.session_head.sequence += 1;
            oracle.apply(&state, &accept).map(|_| ())
        }
        NegativeKindV1::ExpiredProposalValidity => {
            let mut action = action_from_stimulus(accept)?;
            set_admitted_at(&mut action, 850);
            oracle.apply(&state, &Stimulus::Action(action)).map(|_| ())
        }
        NegativeKindV1::ExpiredApproval => {
            let mut action = action_from_stimulus(accept)?;
            set_admitted_at(&mut action, 500);
            oracle.apply(&state, &Stimulus::Action(action)).map(|_| ())
        }
        NegativeKindV1::WrongSigner => {
            let mut action = action_from_stimulus(accept)?;
            if let Action::AcceptCurrentProposal { acceptance, .. } = &mut action {
                acceptance.signature.signer_role = Role::SellerAgent;
            }
            oracle.apply(&state, &Stimulus::Action(action)).map(|_| ())
        }
        NegativeKindV1::RoleAuthorityViolation => {
            let mut action = action_from_stimulus(accept)?;
            set_actor(&mut action, Role::SellerAgent);
            oracle.apply(&state, &Stimulus::Action(action)).map(|_| ())
        }
        NegativeKindV1::DeadlineAtEquality => {
            let deadline_state = oracle
                .apply(
                    &Oracle::initialize(FORMATION_DEADLINE),
                    &Stimulus::TimerFired(TimerFired {
                        timer: TimerKind::FormationDeadline,
                        generation: 1,
                        scheduled_for: FORMATION_DEADLINE,
                        fired_at: FORMATION_DEADLINE,
                        expected_object_id: None,
                    }),
                )?
                .state;
            let event = stream_event(
                &deadline_state,
                "evt_transaction_deadline_equal",
                "transaction",
                "deadline.elapsed",
                Role::VenueSigner,
            )?;
            oracle
                .apply(
                    &deadline_state,
                    &Stimulus::Action(Action::RecordTransactionDeadlineElapsed {
                        action_id: "act_deadline_equal".to_owned(),
                        actor: Role::VenueSigner,
                        basis: deadline_state.basis(),
                        admitted_at: FORMATION_DEADLINE,
                        occurred_at: FORMATION_DEADLINE,
                        deadline_event: event,
                    }),
                )
                .map(|_| ())
        }
        NegativeKindV1::SignedFormationExpiry => run_expiry_path().map(|_| ()),
        NegativeKindV1::PrivacyNoninterference
        | NegativeKindV1::ForbiddenImport
        | NegativeKindV1::ResourceExhaustion
        | NegativeKindV1::CallbackFaultBeforeCommit
        | NegativeKindV1::MissingRetainedBundle
        | NegativeKindV1::CorruptRetainedBundle => Ok(()),
    }
}

pub fn run_expiry_path() -> Result<OracleState, OracleError> {
    let oracle = Oracle;
    let initial = Oracle::initialize(FORMATION_DEADLINE);
    let timer = Stimulus::TimerFired(TimerFired {
        timer: TimerKind::FormationDeadline,
        generation: 1,
        scheduled_for: FORMATION_DEADLINE,
        fired_at: FORMATION_DEADLINE,
        expected_object_id: None,
    });
    let mut state = oracle.apply(&initial, &timer)?.state;
    let deadline_event = stream_event(
        &state,
        "evt_transaction_deadline_04",
        "transaction",
        "deadline.elapsed",
        Role::VenueSigner,
    )?;
    state = oracle
        .apply(
            &state,
            &Stimulus::Action(Action::RecordTransactionDeadlineElapsed {
                action_id: "act_expire_transaction".to_owned(),
                actor: Role::VenueSigner,
                basis: state.basis(),
                admitted_at: FORMATION_DEADLINE + 2,
                occurred_at: FORMATION_DEADLINE + 1,
                deadline_event,
            }),
        )?
        .state;
    let close_event = stream_event(
        &state,
        "evt_session_close_02",
        "session",
        "session.closed",
        Role::VenueSigner,
    )?;
    state = oracle
        .apply(
            &state,
            &Stimulus::Action(Action::CloseTransactionExpiredSession {
                action_id: "act_close_expired_session".to_owned(),
                actor: Role::VenueSigner,
                basis: state.basis(),
                admitted_at: FORMATION_DEADLINE + 3,
                close_event,
            }),
        )?
        .state;
    Ok(state)
}

fn proposal(
    state: &OracleState,
    author: Role,
    offer_id: &str,
    supersedes: Option<&str>,
    valid_until: u64,
    amount: &str,
) -> Result<crate::Proposal, OracleError> {
    let offer = object(
        offer_id,
        "offer",
        author,
        "offer_submission",
        json!({
            "offeree": if author == Role::BuyerAgent { "org_delta" } else { "org_northstar" },
            "offeror": if author == Role::BuyerAgent { "org_northstar" } else { "org_delta" },
            "session_id": state.session_id,
            "supersedes_offer_id": supersedes,
            "terms": {
                "core": {
                    "description": "Calibration and digital certificates for 20 pressure transmitters",
                    "quantity": "20",
                    "total": {"amount": amount, "currency": "EUR"},
                    "unit_code": "H87",
                    "unit_name": "piece"
                },
                "profile": "a202-profile/calibration-service/0.1",
                "profile_terms": {
                    "acceptance": {"certificate_required": true, "machine_readable_result_required": true, "qualification_standard": "ISO/IEC 17025:2017"},
                    "completion": {"business_calendar": "NL", "business_days_after_collection": 15},
                    "payment": {"balance_trigger": "buyer_acceptance", "prepayment_percent": "20"},
                    "rework": {"included_attempts": 1}
                }
            },
            "valid_until": valid_until,
        }),
    )?;
    let session_event = stream_event(
        state,
        &format!("evt_session_offer_{:02}", state.session_head.sequence + 1),
        "session",
        "offer.submitted",
        Role::VenueSigner,
    )?;
    Ok(crate::Proposal {
        author,
        offer,
        session_event,
        valid_until,
        supersedes_offer_id: supersedes.map(str::to_owned),
    })
}

fn approval_binding(state: &OracleState, expires_at: u64) -> Result<ApprovalBinding, OracleError> {
    let proposal = state
        .current_proposal
        .as_ref()
        .ok_or_else(|| OracleError::new(RejectionCode::InvalidPhase, "proposal fixture missing"))?;
    let candidate = canonical_bytes(&json!({
        "action": "offer.accept",
        "expected_sequence": state.session_head.sequence + 1,
        "offer_hash": proposal.offer.declared_content_hash,
        "offer_id": proposal.offer.object_id,
        "session_id": state.session_id,
        "transaction_id": state.transaction_id,
    }))?;
    let candidate_canonical_json = String::from_utf8(candidate).map_err(|error| {
        OracleError::new(
            RejectionCode::NonCanonicalA202Bytes,
            format!("fixture is not UTF-8: {error}"),
        )
    })?;
    Ok(ApprovalBinding {
        candidate_wire_digest: tagged_blake3(candidate_canonical_json.as_bytes()),
        candidate_canonical_json,
        transaction_id: state.transaction_id.clone(),
        proposal_id: proposal.offer.object_id.clone(),
        proposal_content_hash: proposal.offer.declared_content_hash.clone(),
        room_head_at_request: state.room_head.clone(),
        transaction_head_at_request: state.transaction_head.clone(),
        session_head_at_request: state.session_head.clone(),
        approver_id: "principal:northstar:procurement_director".to_owned(),
        expires_at,
    })
}

fn exact_approval(
    binding: &ApprovalBinding,
    decision: ApprovalDecision,
) -> Result<ExactApproval, OracleError> {
    let approval = object(
        "apr_worldstream_exact_01",
        "approval",
        Role::BuyerApprover,
        "object_issuance",
        json!({
            "action_hash": binding.candidate_wire_digest,
            "approver": binding.approver_id,
            "decision": if decision == ApprovalDecision::Approved { "approved" } else { "rejected" },
            "expires_at": binding.expires_at,
            "proposal_hash": binding.proposal_content_hash,
            "room_head": binding.room_head_at_request,
            "session_head": binding.session_head_at_request,
            "transaction_head": binding.transaction_head_at_request,
        }),
    )?;
    Ok(ExactApproval {
        binding: binding.clone(),
        decision,
        approval,
    })
}

fn agreement_json(state: &OracleState) -> Result<String, OracleError> {
    let bytes = canonical_bytes(&json!({
        "accepted_offer_hash": state.accepted_offer_hash,
        "accepted_offer_id": state.accepted_offer_id,
        "agreement_id": "agr_worldstream_01",
        "buyer": "org_northstar",
        "object_type": "agreement",
        "session_id": state.session_id,
        "spec_version": "a202-commercial/0.1",
        "supplier": "org_delta",
        "transaction_id": state.transaction_id,
    }))?;
    String::from_utf8(bytes).map_err(|error| {
        OracleError::new(
            RejectionCode::NonCanonicalA202Bytes,
            format!("agreement fixture is not UTF-8: {error}"),
        )
    })
}

fn stream_event(
    state: &OracleState,
    event_id: &str,
    stream_kind: &str,
    event_type: &str,
    signer: Role,
) -> Result<ExactA202Object, OracleError> {
    let (sequence, previous_event_hash, stream_id) = if stream_kind == "session" {
        (
            state.session_head.sequence + 1,
            state.session_head.event_hash.clone(),
            state.session_id.clone(),
        )
    } else {
        (
            state.transaction_head.sequence + 1,
            state.transaction_head.event_hash.clone(),
            state.transaction_id.clone(),
        )
    };
    object(
        event_id,
        "transaction_event",
        signer,
        "event_append",
        json!({
            "event_type": event_type,
            "previous_event_hash": previous_event_hash,
            "sequence": sequence,
            "stream": {"id": stream_id, "kind": stream_kind},
        }),
    )
}

fn object(
    object_id: &str,
    object_type: &str,
    signer: Role,
    purpose: &str,
    payload: serde_json::Value,
) -> Result<ExactA202Object, OracleError> {
    let declared_hash = content_hash(object_id);
    let bytes = canonical_bytes(&json!({
        "content_hash": declared_hash,
        "id": object_id,
        "object_type": object_type,
        "payload": payload,
        "spec_version": "a202-commercial/0.1",
        "transaction_id": "txn_calibration_worldstream_01",
        "version": 1,
    }))?;
    let canonical_json = String::from_utf8(bytes).map_err(|error| {
        OracleError::new(
            RejectionCode::NonCanonicalA202Bytes,
            format!("fixture is not UTF-8: {error}"),
        )
    })?;
    Ok(exact_object_from_canonical(
        object_id,
        object_type,
        declared_hash,
        canonical_json,
        signer,
        purpose,
    ))
}

fn exact_object_from_canonical(
    object_id: &str,
    object_type: &str,
    declared_content_hash: String,
    canonical_json: String,
    signer: Role,
    purpose: &str,
) -> ExactA202Object {
    let wire_digest = tagged_blake3(canonical_json.as_bytes());
    ExactA202Object {
        object_id: object_id.to_owned(),
        object_type: object_type.to_owned(),
        declared_content_hash,
        canonical_json,
        wire_digest: wire_digest.clone(),
        signature: fixture_signature(signer, purpose, &wire_digest),
    }
}

fn content_hash(seed: &str) -> String {
    blake3::hash(format!("a202-fixture-content/v1\0{seed}").as_bytes())
        .to_hex()
        .to_string()
}

fn apply_and_push(
    oracle: &Oracle,
    state: &mut OracleState,
    stimuli: &mut Vec<Stimulus>,
    action: Action,
) -> Result<(), OracleError> {
    let stimulus = Stimulus::Action(action);
    *state = oracle.apply(state, &stimulus)?.state;
    stimuli.push(stimulus);
    Ok(())
}

fn action_from_stimulus(stimulus: Stimulus) -> Result<Action, OracleError> {
    match stimulus {
        Stimulus::Action(action) => Ok(action),
        Stimulus::TimerFired(_) => Err(OracleError::new(
            RejectionCode::InvalidPhase,
            "expected Action fixture",
        )),
    }
}

fn action_basis_mut(action: &mut Action) -> &mut crate::ActionBasis {
    match action {
        Action::SubmitProposalRevision { basis, .. }
        | Action::WithdrawLiveProposal { basis, .. }
        | Action::RequestExactApproval { basis, .. }
        | Action::RecordExactApproval { basis, .. }
        | Action::AcceptCurrentProposal { basis, .. }
        | Action::SelectAcceptedProposal { basis, .. }
        | Action::RecordAgreementSignature { basis, .. }
        | Action::CommitAgreement { basis, .. }
        | Action::RecordSessionDeadlineElapsed { basis, .. }
        | Action::RecordTransactionDeadlineElapsed { basis, .. }
        | Action::CloseTransactionExpiredSession { basis, .. } => basis,
    }
}

fn set_admitted_at(action: &mut Action, value: u64) {
    match action {
        Action::SubmitProposalRevision { admitted_at, .. }
        | Action::WithdrawLiveProposal { admitted_at, .. }
        | Action::RequestExactApproval { admitted_at, .. }
        | Action::RecordExactApproval { admitted_at, .. }
        | Action::AcceptCurrentProposal { admitted_at, .. }
        | Action::SelectAcceptedProposal { admitted_at, .. }
        | Action::RecordAgreementSignature { admitted_at, .. }
        | Action::CommitAgreement { admitted_at, .. }
        | Action::RecordSessionDeadlineElapsed { admitted_at, .. }
        | Action::RecordTransactionDeadlineElapsed { admitted_at, .. }
        | Action::CloseTransactionExpiredSession { admitted_at, .. } => *admitted_at = value,
    }
}

fn set_actor(action: &mut Action, value: Role) {
    match action {
        Action::SubmitProposalRevision { actor, .. }
        | Action::WithdrawLiveProposal { actor, .. }
        | Action::RequestExactApproval { actor, .. }
        | Action::RecordExactApproval { actor, .. }
        | Action::AcceptCurrentProposal { actor, .. }
        | Action::SelectAcceptedProposal { actor, .. }
        | Action::RecordAgreementSignature { actor, .. }
        | Action::CommitAgreement { actor, .. }
        | Action::RecordSessionDeadlineElapsed { actor, .. }
        | Action::RecordTransactionDeadlineElapsed { actor, .. }
        | Action::CloseTransactionExpiredSession { actor, .. } => *actor = value,
    }
}

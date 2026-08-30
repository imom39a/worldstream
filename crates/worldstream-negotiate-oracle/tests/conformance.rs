use std::collections::BTreeSet;

use worldstream_negotiate_oracle::{
    ACTIONS, Action, ActionKind, ApprovalDecision, CORPUS_FIXTURE_BYTES, NegativeKindV1, Oracle,
    PERSONAS, PHASES, PRIVACY_MATRIX, Phase, PrivacyExposure, PrivacyProbe, ROLES, RejectionCode,
    Role, Stimulus, canonical_bytes, corpus, corpus_bytes, golden_corpus, golden_plan,
    negative_cases, privacy_projection, run_expiry_path, run_negative, tagged_blake3,
};

#[test]
fn freezes_the_exact_public_vocabulary() {
    assert_eq!(ROLES.len(), 4);
    assert_eq!(PHASES.len(), 8);
    assert_eq!(ACTIONS.len(), 11);
    assert_eq!(worldstream_negotiate_oracle::ATTENTION_REASONS.len(), 4);
    assert_eq!(PERSONAS.len(), 6);
    assert_eq!(
        ROLES.map(Role::as_str),
        [
            "buyer_agent",
            "seller_agent",
            "buyer_approver",
            "venue_signer"
        ]
    );
    assert_eq!(PHASES[0], Phase::FormationOpen);
    assert_eq!(PHASES[7], Phase::Expired);
    assert_eq!(ACTIONS[0], ActionKind::SubmitProposalRevision);
    assert_eq!(ACTIONS[10], ActionKind::CloseTransactionExpiredSession);
}

#[test]
fn golden_path_survives_restart_and_reconnect_byte_for_byte()
-> Result<(), Box<dyn std::error::Error>> {
    let plan = golden_plan()?;
    let oracle = Oracle;
    let mut state = Oracle::initialize(plan.formation_deadline);
    let mut before_restart = None;
    for (index, stimulus) in plan.stimuli.iter().enumerate() {
        state = oracle.apply(&state, stimulus)?.state;
        if index + 1 == plan.restart_after_step {
            let bytes = canonical_bytes(&state)?;
            let restored: worldstream_negotiate_oracle::OracleState =
                serde_json::from_slice(&bytes)?;
            assert_eq!(restored, state);
            assert_eq!(
                oracle.offers(&restored, Role::BuyerAgent),
                oracle.offers(&state, Role::BuyerAgent)
            );
            before_restart = Some(bytes);
            state = restored;
        }
    }
    assert!(before_restart.is_some());
    assert_eq!(state.phase, Phase::Complete);
    assert_eq!(state.aggregate_state, "committed");
    assert_eq!(state.session_state, "accepted");
    assert_eq!(state.room_head.sequence, 9);
    assert_eq!(state.transaction_head.sequence, 5);
    assert_eq!(state.session_head.sequence, 4);

    let mut replay = Oracle::initialize(plan.formation_deadline);
    for stimulus in &plan.stimuli {
        replay = oracle.apply(&replay, stimulus)?.state;
    }
    assert_eq!(canonical_bytes(&replay)?, canonical_bytes(&state)?);
    Ok(())
}

#[test]
fn approval_binds_exact_bytes_hashes_heads_approver_and_expiry()
-> Result<(), Box<dyn std::error::Error>> {
    let plan = golden_plan()?;
    let Some(Stimulus::Action(Action::RequestExactApproval {
        binding: request, ..
    })) = plan.stimuli.get(2)
    else {
        return Err("golden request_exact_approval fixture missing".into());
    };
    let Some(Stimulus::Action(Action::RecordExactApproval {
        approval: recorded, ..
    })) = plan.stimuli.get(3)
    else {
        return Err("golden record_exact_approval fixture missing".into());
    };
    assert_eq!(recorded.decision, ApprovalDecision::Approved);
    assert_eq!(&recorded.binding, request);
    assert_eq!(
        request.candidate_wire_digest,
        tagged_blake3(request.candidate_canonical_json.as_bytes())
    );
    assert!(request.room_head_at_request.sequence > 0);
    assert!(request.transaction_head_at_request.sequence > 0);
    assert!(request.session_head_at_request.sequence > 0);
    assert_eq!(
        request.approver_id,
        "principal:northstar:procurement_director"
    );
    assert!(request.expires_at > 0);
    Ok(())
}

#[test]
fn cross_index_names_every_exact_object_at_its_committed_room_head()
-> Result<(), Box<dyn std::error::Error>> {
    let golden = golden_corpus()?;
    assert_eq!(golden.evidence_cross_index, golden.final_state.evidence);
    assert!(golden.evidence_cross_index.len() >= 10);
    let object_ids: BTreeSet<_> = golden
        .evidence_cross_index
        .iter()
        .map(|entry| entry.object_id.as_str())
        .collect();
    assert_eq!(object_ids.len(), golden.evidence_cross_index.len());
    for entry in &golden.evidence_cross_index {
        assert!(entry.room_sequence > 0);
        assert!(entry.room_digest.starts_with("blake3:"));
        assert!(entry.transaction_head.event_hash.starts_with("sha256:"));
        assert!(entry.session_head.event_hash.starts_with("sha256:"));
        assert!(entry.wire_digest.starts_with("blake3:"));
    }
    Ok(())
}

#[test]
fn all_negative_fixtures_fail_closed_with_the_frozen_code() -> Result<(), Box<dyn std::error::Error>>
{
    for fixture in negative_cases() {
        let result = run_negative(fixture.kind);
        match fixture.expected_code {
            Some(expected) => {
                let error = result.err().ok_or("negative fixture unexpectedly passed")?;
                assert_eq!(error.code, expected, "fixture {}", fixture.id);
            }
            None => assert!(result.is_ok(), "positive control {} failed", fixture.id),
        }
    }
    Ok(())
}

#[test]
fn signed_expiry_establishes_only_the_expired_outcome() -> Result<(), Box<dyn std::error::Error>> {
    let state = run_expiry_path()?;
    assert_eq!(state.phase, Phase::Expired);
    assert_eq!(state.aggregate_state, "expired");
    assert_eq!(state.session_state, "closed");
    assert!(matches!(
        state.outcome,
        Some(worldstream_negotiate_oracle::Outcome::FormationExpired { .. })
    ));
    Ok(())
}

#[test]
fn pairwise_private_mutations_cover_all_six_personas() {
    let baseline = PrivacyProbe::fixture();
    let mut comparisons = 0;
    for source in PERSONAS {
        for viewer in PERSONAS {
            if source == viewer {
                continue;
            }
            let before = privacy_projection(viewer, &baseline);
            let mut mutated = baseline.clone();
            mutated
                .private_by_persona
                .insert(source, format!("{}-mutated-private-probe", source.as_str()));
            let after = privacy_projection(viewer, &mutated);
            assert_eq!(before, after, "{source:?} leaked to {viewer:?}");
            comparisons += 1;
        }
    }
    assert_eq!(comparisons, 30);
}

#[test]
fn privacy_matrix_denies_spectator_terms_and_seller_acceptance_bytes() {
    let acceptance = PRIVACY_MATRIX
        .iter()
        .find(|row| {
            row.class == worldstream_negotiate_oracle::PrivacyClass::BuyerAcceptanceEnvelope
        })
        .copied();
    assert!(acceptance.is_some());
    if let Some(row) = acceptance {
        assert_eq!(
            row.exposure(worldstream_negotiate_oracle::Persona::SellerAgent),
            PrivacyExposure::None
        );
        assert_eq!(
            row.exposure(worldstream_negotiate_oracle::Persona::BuyerApprover),
            PrivacyExposure::Full
        );
    }
    for row in PRIVACY_MATRIX {
        if row.class != worldstream_negotiate_oracle::PrivacyClass::PublicStatus {
            assert_eq!(
                row.exposure(worldstream_negotiate_oracle::Persona::Spectator),
                PrivacyExposure::None
            );
        }
    }
}

#[test]
fn corpus_is_canonical_machine_readable_and_stable() -> Result<(), Box<dyn std::error::Error>> {
    let first = corpus_bytes()?;
    let second = corpus_bytes()?;
    assert_eq!(first, second);
    let decoded: worldstream_negotiate_oracle::CorpusV1 = serde_json::from_slice(&first)?;
    assert_eq!(decoded, corpus()?);
    assert_eq!(decoded.privacy_mutations.len(), 30);
    assert!(matches!(
        decoded.signed_expiry.outcome,
        worldstream_negotiate_oracle::Outcome::FormationExpired { .. }
    ));
    let checked_in: worldstream_negotiate_oracle::CorpusV1 =
        serde_json::from_slice(CORPUS_FIXTURE_BYTES)?;
    assert_eq!(checked_in, decoded);
    assert_eq!(
        tagged_blake3(&first),
        "blake3:84cdd48a826df697c8a02fa72e344bbe45a43151d3a2eb2bebc92533a316cb78"
    );
    Ok(())
}

#[test]
fn oracle_sources_have_no_runtime_io_authority() {
    let sources = [
        include_str!("../src/model.rs"),
        include_str!("../src/oracle.rs"),
        include_str!("../src/privacy.rs"),
        include_str!("../src/fixtures.rs"),
    ];
    for source in sources {
        assert!(!source.contains("std::fs"));
        assert!(!source.contains("std::net"));
        assert!(!source.contains("std::time"));
        assert!(!source.contains("std::process"));
        assert!(!source.contains("rand::"));
    }
}

#[test]
fn changed_a202_heads_are_independent_from_room_head() -> Result<(), Box<dyn std::error::Error>> {
    let golden = golden_corpus()?;
    let after_approval = golden.steps.get(3).ok_or("approval checkpoint missing")?;
    let after_acceptance = golden.steps.get(4).ok_or("acceptance checkpoint missing")?;
    assert_eq!(
        after_approval.expected.transaction_head,
        after_acceptance.expected.transaction_head
    );
    assert_ne!(
        after_approval.expected.session_head,
        after_acceptance.expected.session_head
    );
    assert_ne!(
        after_approval.expected.room_head,
        after_acceptance.expected.room_head
    );
    assert_eq!(
        run_negative(NegativeKindV1::ChangedSessionHead)
            .err()
            .map(|error| error.code),
        Some(RejectionCode::StaleSessionHead)
    );
    Ok(())
}

#[test]
fn offers_are_derived_only_from_phase_role_and_exact_current_facts()
-> Result<(), Box<dyn std::error::Error>> {
    let plan = golden_plan()?;
    let oracle = Oracle;
    let mut state = Oracle::initialize(plan.formation_deadline);
    assert_eq!(
        oracle.offers(&state, Role::BuyerAgent),
        vec![ActionKind::SubmitProposalRevision]
    );
    assert_eq!(
        oracle.offers(&state, Role::SellerAgent),
        vec![ActionKind::SubmitProposalRevision]
    );

    state = oracle.apply(&state, &plan.stimuli[0])?.state;
    assert_eq!(
        oracle.offers(&state, Role::BuyerAgent),
        vec![ActionKind::WithdrawLiveProposal]
    );
    assert_eq!(
        oracle.offers(&state, Role::SellerAgent),
        vec![ActionKind::SubmitProposalRevision]
    );

    state = oracle.apply(&state, &plan.stimuli[1])?.state;
    assert_eq!(
        oracle.offers(&state, Role::BuyerAgent),
        vec![
            ActionKind::SubmitProposalRevision,
            ActionKind::RequestExactApproval
        ]
    );
    assert_eq!(
        oracle.offers(&state, Role::SellerAgent),
        vec![ActionKind::WithdrawLiveProposal]
    );

    state = oracle.apply(&state, &plan.stimuli[2])?.state;
    assert_eq!(
        oracle.offers(&state, Role::BuyerApprover),
        vec![ActionKind::RecordExactApproval]
    );
    state = oracle.apply(&state, &plan.stimuli[3])?.state;
    assert!(
        oracle
            .offers(&state, Role::BuyerAgent)
            .contains(&ActionKind::AcceptCurrentProposal)
    );

    for stimulus in plan.stimuli.iter().skip(4) {
        state = oracle.apply(&state, stimulus)?.state;
    }
    for role in ROLES {
        assert!(oracle.offers(&state, role).is_empty());
    }
    Ok(())
}

#[test]
fn proposal_and_approval_timers_invalidate_only_the_named_live_fact()
-> Result<(), Box<dyn std::error::Error>> {
    let plan = golden_plan()?;
    let oracle = Oracle;
    let mut state = Oracle::initialize(plan.formation_deadline);
    state = oracle.apply(&state, &plan.stimuli[0])?.state;
    state = oracle.apply(&state, &plan.stimuli[1])?.state;
    let proposal = state
        .current_proposal
        .as_ref()
        .ok_or("current proposal missing")?;
    let proposal_timer = worldstream_negotiate_oracle::TimerFired {
        timer: worldstream_negotiate_oracle::TimerKind::ProposalValidity,
        generation: 1,
        scheduled_for: proposal.valid_until,
        fired_at: proposal.valid_until,
        expected_object_id: Some(proposal.offer.object_id.clone()),
    };
    let expired = oracle
        .apply(&state, &Stimulus::TimerFired(proposal_timer))?
        .state;
    assert!(expired.proposal_expired);
    assert_eq!(expired.phase, Phase::FormationOpen);
    assert!(
        !oracle
            .offers(&expired, Role::BuyerAgent)
            .contains(&ActionKind::RequestExactApproval)
    );

    state = oracle.apply(&state, &plan.stimuli[2])?.state;
    let binding = state
        .pending_approval
        .as_ref()
        .ok_or("pending approval missing")?;
    let approval_timer = worldstream_negotiate_oracle::TimerFired {
        timer: worldstream_negotiate_oracle::TimerKind::ApprovalExpiry,
        generation: 1,
        scheduled_for: binding.expires_at,
        fired_at: binding.expires_at,
        expected_object_id: Some(binding.candidate_wire_digest.clone()),
    };
    let expired = oracle
        .apply(&state, &Stimulus::TimerFired(approval_timer))?
        .state;
    assert!(expired.pending_approval.is_none());
    assert!(expired.recorded_approval.is_none());
    assert_eq!(expired.phase, Phase::FormationOpen);
    Ok(())
}

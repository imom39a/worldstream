#![allow(clippy::panic)]

use super::*;

#[test]
fn attention_normalization_and_context_preparation_are_pure() {
    let signal = CanonicalJsonV1::parse(
        br#"{"deduplication_key":"phase:1","priority":7,"reason":"ready","target_member_id":"01ARZ3NDEKTSV4RRFFQ69G5FC0"}"#,
    )
    .unwrap_or_else(|error| panic!("canonical Attention: {error}"));
    let attention = ActivationAttentionV1::from_canonical(&signal)
        .unwrap_or_else(|error| panic!("Attention shape: {error}"));
    assert_eq!(attention.reason_code, "ready");
    assert_eq!(attention.priority, 7);

    let head_bytes = CanonicalJsonV1::parse(
        br#"{"activity_state_hash":"blake3:0000000000000000000000000000000000000000000000000000000000000000","authoritative_state_hash":"blake3:0000000000000000000000000000000000000000000000000000000000000000","core_schema_version":"worldstream.core-room-state.v1","core_state_hash":"blake3:0000000000000000000000000000000000000000000000000000000000000000","genesis_or_transition_hash":"blake3:0000000000000000000000000000000000000000000000000000000000000000","pack_digest":"blake3:0000000000000000000000000000000000000000000000000000000000000000","room_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","room_seq":0}"#,
    )
    .unwrap_or_else(|error| panic!("canonical Head: {error}"))
    .to_bytes()
    .unwrap_or_else(|error| panic!("Head bytes: {error}"));
    let head = CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&head_bytes)
        .unwrap_or_else(|error| panic!("Head shape: {error}"));
    let context = prepare_activation_context(ActivationContextInputV1 {
        activation_id: "activation:1".to_owned(),
        claim_id: "claim:1".to_owned(),
        cause_room_seq: RoomSequenceV1::new(1).unwrap_or_else(|error| panic!("sequence: {error}")),
        reason_code: "ready".to_owned(),
        lease_generation: 1,
        lease_until: "2026-08-15T12:00:30Z".to_owned(),
        semantic_deadline: None,
        room_head: head,
        integrity_generation: 1,
        policy_revision: 1,
        authority_generation: 1,
        membership_generation: 1,
        frame_head: 0,
        retained_floor: 1,
        cursor: None,
        projection_schema: PROJECTION_SCHEMA_V1.to_owned(),
        projection_bytes: br"{}".to_vec(),
        action_offers_bytes: br"[]".to_vec(),
        runner_budget_bytes: br"{}".to_vec(),
        runner_limits_bytes: br"{}".to_vec(),
        artifact_references: Vec::new(),
        delivery: ActivationDeliveryV1::ProjectionReset {
            baseline_frame_head: 0,
            reason: "first_attach".to_owned(),
        },
    })
    .unwrap_or_else(|error| panic!("exact context: {error}"));
    assert_eq!(
        context.delivery,
        ActivationDeliveryV1::ProjectionReset {
            baseline_frame_head: 0,
            reason: "first_attach".to_owned(),
        }
    );
    assert_ne!(
        context
            .context_hash()
            .unwrap_or_else(|error| panic!("context hash: {error}")),
        Blake3DigestV1::hash(&[])
    );
}

#[test]
fn attention_accepts_pack_declared_action_types() {
    let signal = CanonicalJsonV1::parse(
        br#"{"action_types":["endorse_plan","challenge_plan"],"deduplication_key":"endorsement_requested:broker:2","deadline":"2026-08-15T14:00:30Z","priority":1,"reason":"endorsement_requested","target_member_id":"01ARZ3NDEKTSV4RRFFQ69G5FC2"}"#,
    )
    .unwrap_or_else(|error| panic!("canonical Heist Attention: {error}"));

    let attention = ActivationAttentionV1::from_canonical(&signal)
        .unwrap_or_else(|error| panic!("Heist Attention shape: {error}"));

    assert_eq!(attention.action_types, ["endorse_plan", "challenge_plan"]);
    let decision = PreparedActivationDecisionV1::from_attention(
        &signal,
        RoomSequenceV1::new(1).unwrap_or_else(|error| panic!("room sequence: {error}")),
    )
    .unwrap_or_else(|error| panic!("Heist Activation decision: {error}"));
    assert!(
        decision
            .canonical_decision_bytes()
            .windows(b"action_types".len())
            .any(|window| window == b"action_types")
    );
    let decoded = CanonicalJsonV1::decode_canonical::<ActivationDecisionV1>(
        decision.canonical_decision_bytes(),
    )
    .unwrap_or_else(|error| panic!("Activation decision shape: {error}"));
    let activation_id = decoded
        .activation_id
        .unwrap_or_else(|| panic!("Activation intent identity"));
    assert!(
        activation_id
            .parse::<worldstream_protocol::UlidString>()
            .is_ok()
    );
    assert!(!activation_id.contains(decoded.attention.target_member_id.as_str()));
    assert_eq!(
        activation_id,
        activation_id_for_attention_v1(
            decoded.cause_room_seq,
            &decoded.attention.target_member_id,
            &decoded.attention.deduplication_key,
        )
        .unwrap_or_else(|error| panic!("recovered Activation identity: {error}"))
    );
}

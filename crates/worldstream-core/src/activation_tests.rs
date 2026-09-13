#![allow(clippy::panic)]

use super::*;

fn activation_context_input(
    projection_bytes: Vec<u8>,
    delivery: ActivationDeliveryV1,
    frame_head: u64,
    cursor: Option<u64>,
) -> ActivationContextInputV1 {
    let head_bytes = CanonicalJsonV1::parse(
        br#"{"activity_state_hash":"blake3:0000000000000000000000000000000000000000000000000000000000000000","authoritative_state_hash":"blake3:0000000000000000000000000000000000000000000000000000000000000000","core_schema_version":"worldstream.core-room-state.v1","core_state_hash":"blake3:0000000000000000000000000000000000000000000000000000000000000000","genesis_or_transition_hash":"blake3:0000000000000000000000000000000000000000000000000000000000000000","pack_digest":"blake3:0000000000000000000000000000000000000000000000000000000000000000","room_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV","room_seq":0}"#,
    )
    .unwrap_or_else(|error| panic!("canonical Head: {error}"))
    .to_bytes()
    .unwrap_or_else(|error| panic!("Head bytes: {error}"));
    ActivationContextInputV1 {
        activation_id: "activation:1".to_owned(),
        claim_id: "claim:1".to_owned(),
        cause_room_seq: RoomSequenceV1::new(1).unwrap_or_else(|error| panic!("sequence: {error}")),
        reason_code: "ready".to_owned(),
        lease_generation: 1,
        lease_until: "2026-08-15T12:00:30Z".to_owned(),
        semantic_deadline: None,
        room_head: CanonicalJsonV1::decode_canonical::<CompleteHeadV1>(&head_bytes)
            .unwrap_or_else(|error| panic!("Head shape: {error}")),
        integrity_generation: 1,
        policy_revision: 1,
        authority_generation: 1,
        membership_generation: 1,
        frame_head,
        retained_floor: 1,
        cursor,
        projection_schema: PROJECTION_SCHEMA_V1.to_owned(),
        projection_bytes,
        action_offers_bytes: br"[]".to_vec(),
        runner_budget_bytes: br"{}".to_vec(),
        runner_limits_bytes: br"{}".to_vec(),
        artifact_references: Vec::new(),
        delivery,
    }
}

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
fn invocation_context_byte_limit_accepts_the_exact_boundary_and_rejects_one_less() {
    let input = activation_context_input(
        br"{}".to_vec(),
        ActivationDeliveryV1::ProjectionReset {
            baseline_frame_head: 0,
            reason: "first_attach".to_owned(),
        },
        0,
        None,
    );
    let exact = prepare_activation_context_with_byte_limit(input.clone(), usize::MAX)
        .unwrap_or_else(|error| panic!("unbounded context: {error}"))
        .canonical_bytes()
        .unwrap_or_else(|error| panic!("context bytes: {error}"))
        .len();
    assert!(prepare_activation_context_with_byte_limit(input.clone(), exact).is_ok());
    assert_eq!(
        prepare_activation_context_with_byte_limit(input, exact - 1),
        Err(ActivationContextErrorV1::TooLarge)
    );
}

#[test]
fn invocation_context_aggregate_bound_handles_many_small_frames_and_large_projection() {
    let frames = (1..=512)
        .map(|frame_seq| ActivationFrameV1 {
            frame_seq,
            cause_room_seq: RoomSequenceV1::new(1)
                .unwrap_or_else(|error| panic!("sequence: {error}")),
            payload_hash: Blake3DigestV1::hash(br"{}"),
            payload_bytes: br"{}".to_vec(),
        })
        .collect();
    let context = prepare_activation_context(activation_context_input(
        br"{}".to_vec(),
        ActivationDeliveryV1::RetainedFrames {
            cursor_exclusive: 0,
            through_frame_head: 512,
            frames,
        },
        512,
        Some(0),
    ))
    .unwrap_or_else(|error| panic!("many small frames: {error}"));
    assert!(
        context
            .canonical_bytes()
            .unwrap_or_else(|error| panic!("context bytes: {error}"))
            .len()
            <= MAX_ACTIVATION_INVOCATION_CONTEXT_BYTES
    );

    let oversized_projection = format!(
        "\"{}\"",
        "x".repeat(MAX_ACTIVATION_INVOCATION_CONTEXT_BYTES)
    );
    assert_eq!(
        prepare_activation_context(activation_context_input(
            oversized_projection.into_bytes(),
            ActivationDeliveryV1::ProjectionReset {
                baseline_frame_head: 0,
                reason: "first_attach".to_owned(),
            },
            0,
            None,
        )),
        Err(ActivationContextErrorV1::TooLarge)
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

#[test]
fn refresh_policy_is_bounded_and_deadline_work_is_an_obligation() {
    assert_eq!(
        ActivationAttentionClassV1::from_semantic_deadline(None),
        ActivationAttentionClassV1::Refreshable
    );
    let deadline = "2026-08-15T14:00:30Z"
        .parse::<TimerScheduledFor>()
        .unwrap_or_else(|error| panic!("deadline: {error}"));
    assert_eq!(
        ActivationAttentionClassV1::from_semantic_deadline(Some(&deadline)),
        ActivationAttentionClassV1::Obligation
    );
    assert!(activation_refresh_budget_allows_v1(63, 1_000, 1_000));
    assert!(!activation_refresh_budget_allows_v1(64, 1_000, 1_000));
    assert!(!activation_refresh_budget_allows_v1(
        1,
        MAX_PENDING_REFRESH_BYTES_V1,
        1
    ));
}

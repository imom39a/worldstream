use super::session::{
    CapturedSessionBarrierV1, SessionBarrierV1, SessionCloseReasonV1, SessionErrorV1,
    SessionFrameV1, SessionPublishOutcomeV1, SessionStateV1, SessionV1,
};
use super::{Blake3DigestV1, CompleteHeadV1};

fn session(buffer_capacity: usize) -> SessionV1 {
    match SessionV1::new(buffer_capacity) {
        Ok(session) => session,
        Err(error) => unreachable!("fixture Session failed: {error:?}"),
    }
}

fn barrier() -> SessionBarrierV1 {
    match SessionBarrierV1::new(complete_head(), 191, 150, Some(184)) {
        Ok(barrier) => barrier,
        Err(error) => unreachable!("fixture barrier failed: {error:?}"),
    }
}

fn first_attach_barrier() -> SessionBarrierV1 {
    match SessionBarrierV1::new(complete_head(), 191, 150, None) {
        Ok(barrier) => barrier,
        Err(error) => unreachable!("fixture first-attach barrier failed: {error:?}"),
    }
}

fn complete_head() -> CompleteHeadV1 {
    let digest = Blake3DigestV1::hash(b"session fixture");
    let pack_digest = Blake3DigestV1::hash(b"session pack");
    let room_id = match "01ARZ3NDEKTSV4RRFFQ69G5FBE".parse() {
        Ok(room_id) => room_id,
        Err(error) => unreachable!("fixture Room ID failed: {error:?}"),
    };
    let room_seq = match super::RoomSequenceV1::new(91) {
        Ok(room_seq) => room_seq,
        Err(error) => unreachable!("fixture Room sequence failed: {error:?}"),
    };
    let pack_digest = match pack_digest.to_string().parse() {
        Ok(pack_digest) => pack_digest,
        Err(error) => unreachable!("fixture Pack digest failed: {error:?}"),
    };

    CompleteHeadV1 {
        room_id,
        room_seq,
        genesis_or_transition_hash: digest.clone(),
        core_schema_version: "worldstream.core-room-state.v1".to_owned(),
        pack_digest,
        core_state_hash: digest.clone(),
        activity_state_hash: digest.clone(),
        authoritative_state_hash: digest,
    }
}

fn frame(frame_seq: u64) -> SessionFrameV1 {
    match SessionFrameV1::new(frame_seq) {
        Ok(frame) => frame,
        Err(error) => unreachable!("fixture frame failed: {error:?}"),
    }
}

fn capture(session: &mut SessionV1) -> CapturedSessionBarrierV1 {
    match session.capture_barrier(barrier()) {
        Ok(captured) => captured,
        Err(error) => unreachable!("fixture capture failed: {error:?}"),
    }
}

#[test]
fn new_session_starts_attaching() {
    assert_eq!(session(2).state(), SessionStateV1::Attaching);
}

#[test]
fn capture_barrier_enters_catching_up_and_redacts_its_opaque_token() {
    let mut session = session(2);
    let captured = capture(&mut session);

    assert_eq!(session.state(), SessionStateV1::CatchingUp);
    assert_eq!(captured.barrier(), barrier());
    assert_eq!(captured.barrier().complete_head(), &complete_head());
    assert_eq!(session.barrier(), Some(barrier()));
    assert_eq!(
        format!("{:?}", captured.sync_token()),
        "SessionSyncTokenV1(REDACTED)"
    );
    assert!(!format!("{session:?}").contains("worldstream/session-sync-token"));
    assert!(!format!("{session:?}").contains("REDACTED"));
}

#[test]
fn sessions_issue_distinct_tokens_for_identical_complete_barriers() {
    let mut first = session(1);
    let first_capture = capture(&mut first);
    let mut second = session(1);
    let second_capture = capture(&mut second);

    assert_ne!(first_capture.sync_token(), second_capture.sync_token());
    assert_eq!(first_capture.barrier(), second_capture.barrier());
}

#[test]
fn first_attach_barrier_has_no_cursor() {
    let barrier = first_attach_barrier();

    assert_eq!(barrier.cursor(), None);
}

#[test]
fn first_attach_sync_ack_does_not_require_a_cursor() {
    let mut session = session(1);
    let captured = match session.capture_barrier(first_attach_barrier()) {
        Ok(captured) => captured,
        Err(error) => unreachable!("fixture first-attach capture failed: {error:?}"),
    };

    assert_eq!(captured.barrier().cursor(), None);
    assert_eq!(session.sync_ack(captured.sync_token(), 191), Ok(Vec::new()));
}

#[test]
fn post_barrier_frames_are_bounded_and_flushed_in_sequence_order() {
    let mut session = session(3);
    let captured = capture(&mut session);

    assert_eq!(
        session.publish(frame(192)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );
    assert_eq!(
        session.publish(frame(193)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );
    assert_eq!(
        session.publish(frame(194)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );

    let flushed = match session.sync_ack(captured.sync_token(), 191) {
        Ok(frames) => frames,
        Err(error) => unreachable!("fixture ACK failed: {error:?}"),
    };
    assert_eq!(
        flushed
            .iter()
            .map(|frame| frame.frame_seq())
            .collect::<Vec<_>>(),
        vec![192, 193, 194]
    );
    assert_eq!(session.state(), SessionStateV1::Live);
}

#[test]
fn catching_up_rejects_gaps_and_duplicates_without_reordering_delivery() {
    let mut session = session(3);
    let captured = capture(&mut session);

    assert_eq!(
        session.publish(frame(193)),
        Err(SessionErrorV1::FrameSequenceGap)
    );
    assert_eq!(
        session.publish(frame(192)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );
    assert_eq!(
        session.publish(frame(192)),
        Err(SessionErrorV1::DuplicateBufferedFrame)
    );
    assert_eq!(
        session.publish(frame(194)),
        Err(SessionErrorV1::FrameSequenceGap)
    );
    assert_eq!(
        session.publish(frame(193)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );
    assert_eq!(
        session.publish(frame(194)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );

    let flushed = match session.sync_ack(captured.sync_token(), 191) {
        Ok(frames) => frames,
        Err(error) => unreachable!("fixture ACK failed: {error:?}"),
    };
    assert_eq!(
        flushed
            .iter()
            .map(|frame| frame.frame_seq())
            .collect::<Vec<_>>(),
        vec![192, 193, 194]
    );
}

#[test]
fn catching_up_rejects_stale_publication() {
    let mut session = session(2);
    let _captured = capture(&mut session);

    assert_eq!(
        session.publish(frame(191)),
        Err(SessionErrorV1::FrameNotPostBarrier)
    );
    assert_eq!(
        session.publish(frame(192)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );
    assert_eq!(
        session.publish(frame(191)),
        Err(SessionErrorV1::FrameNotPostBarrier)
    );
}

#[test]
fn live_rejects_stale_duplicate_and_out_of_order_publication() {
    let mut session = session(2);
    let captured = capture(&mut session);
    assert_eq!(
        session.publish(frame(192)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );
    assert_eq!(
        session.sync_ack(captured.sync_token(), 191),
        Ok(vec![frame(192)])
    );

    assert_eq!(
        session.publish(frame(192)),
        Err(SessionErrorV1::FrameSequenceStale)
    );
    assert_eq!(
        session.publish(frame(194)),
        Err(SessionErrorV1::FrameSequenceGap)
    );
    assert_eq!(
        session.publish(frame(193)),
        Ok(SessionPublishOutcomeV1::DeliverNow(frame(193)))
    );
    assert_eq!(
        session.publish(frame(193)),
        Err(SessionErrorV1::FrameSequenceStale)
    );
}

#[test]
fn sync_ack_requires_exact_token_and_baseline_without_moving_cursor() {
    let mut first = session(2);
    let first_capture = capture(&mut first);
    let mut second = session(2);
    let second_capture = capture(&mut second);

    assert_eq!(
        second.sync_ack(first_capture.sync_token(), 191),
        Err(SessionErrorV1::SyncTokenMismatch)
    );
    assert_eq!(second.state(), SessionStateV1::CatchingUp);
    assert_eq!(
        second.sync_ack(second_capture.sync_token(), 190),
        Err(SessionErrorV1::SyncBaselineMismatch)
    );
    assert_eq!(second.state(), SessionStateV1::CatchingUp);
    assert_eq!(
        second.barrier().map(|barrier| barrier.cursor()),
        Some(Some(184))
    );
    assert_eq!(
        second.sync_ack(second_capture.sync_token(), 191),
        Ok(Vec::new())
    );
    assert_eq!(
        second.barrier().map(|barrier| barrier.cursor()),
        Some(Some(184))
    );
}

#[test]
fn matching_sync_ack_is_single_use() {
    let mut session = session(1);
    let captured = capture(&mut session);

    assert_eq!(session.sync_ack(captured.sync_token(), 191), Ok(Vec::new()));
    assert_eq!(
        session.sync_ack(captured.sync_token(), 191),
        Err(SessionErrorV1::SyncAlreadyAcknowledged)
    );
}

#[test]
fn buffer_overflow_closes_as_slow_consumer() {
    let mut session = session(1);
    let _captured = capture(&mut session);

    assert_eq!(
        session.publish(frame(192)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );
    assert_eq!(
        session.publish(frame(193)),
        Err(SessionErrorV1::SlowConsumer)
    );
    assert_eq!(session.state(), SessionStateV1::Closed);
    assert_eq!(
        session.close_reason(),
        Some(SessionCloseReasonV1::SlowConsumer)
    );
}

#[test]
fn visibility_loss_closes_and_discards_pending_delivery() {
    let mut session = session(2);
    let captured = capture(&mut session);
    assert_eq!(
        session.publish(frame(192)),
        Ok(SessionPublishOutcomeV1::Buffered)
    );

    assert_eq!(session.close_for_visibility_loss(), Ok(()));
    assert_eq!(session.state(), SessionStateV1::Closed);
    assert_eq!(
        session.close_reason(),
        Some(SessionCloseReasonV1::VisibilityLoss)
    );
    assert_eq!(
        session.sync_ack(captured.sync_token(), 191),
        Err(SessionErrorV1::InvalidState)
    );
}

#[test]
fn live_session_delivers_new_frames_without_buffering() {
    let mut session = session(1);
    let captured = capture(&mut session);
    assert_eq!(session.sync_ack(captured.sync_token(), 191), Ok(Vec::new()));

    assert_eq!(
        session.publish(frame(192)),
        Ok(SessionPublishOutcomeV1::DeliverNow(frame(192)))
    );
}

#[test]
fn invalid_capacity_and_barrier_positions_are_rejected() {
    assert!(matches!(
        SessionV1::new(0),
        Err(SessionErrorV1::ZeroBufferCapacity)
    ));
    assert_eq!(
        SessionBarrierV1::new(complete_head(), 191, 150, Some(192)),
        Err(SessionErrorV1::CursorAheadOfFrameHead)
    );
    assert_eq!(
        SessionBarrierV1::new(complete_head(), 191, 0, Some(184)),
        Err(SessionErrorV1::InvalidRetainedFloor)
    );
    assert!(SessionBarrierV1::new(complete_head(), 191, 192, None).is_ok());
    assert_eq!(
        SessionBarrierV1::new(complete_head(), 191, 193, Some(184)),
        Err(SessionErrorV1::InvalidRetainedFloor)
    );
    assert_eq!(
        SessionFrameV1::new(0),
        Err(SessionErrorV1::InvalidFrameSequence)
    );
}

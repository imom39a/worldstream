//! Storage- and transport-neutral Session delivery state.

use std::{
    collections::BTreeMap,
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::CompleteHeadV1;

static NEXT_SESSION_NONCE: AtomicU64 = AtomicU64::new(1);

/// The four states of one attached Session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionStateV1 {
    /// The Session is admitted but has not captured an attach barrier.
    Attaching,
    /// The Session is delivering its captured range or reset baseline.
    CatchingUp,
    /// The Session has acknowledged its own captured synchronization barrier.
    Live,
    /// The Session cannot receive further delivery.
    Closed,
}

/// The operational reason a Session was closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionCloseReasonV1 {
    /// The bounded post-barrier buffer could not accept another frame.
    SlowConsumer,
    /// The viewer lost authority and incremental delivery was unsafe.
    VisibilityLoss,
}

/// The complete Room and Membership stream positions captured as one attach
/// barrier.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionBarrierV1 {
    complete_head: CompleteHeadV1,
    frame_head: u64,
    retained_floor: u64,
    cursor: Option<u64>,
}

impl SessionBarrierV1 {
    /// Constructs a captured complete Room Head and frame position set.
    ///
    /// # Errors
    ///
    /// Returns an error when the Cursor is ahead of the frame head or the
    /// retained floor is not a valid retained range boundary.
    pub fn new(
        complete_head: CompleteHeadV1,
        frame_head: u64,
        retained_floor: u64,
        cursor: Option<u64>,
    ) -> Result<Self, SessionErrorV1> {
        if cursor.is_some_and(|cursor| cursor > frame_head) {
            return Err(SessionErrorV1::CursorAheadOfFrameHead);
        }
        if retained_floor == 0 || retained_floor > frame_head.saturating_add(1) {
            return Err(SessionErrorV1::InvalidRetainedFloor);
        }
        Ok(Self {
            complete_head,
            frame_head,
            retained_floor,
            cursor,
        })
    }

    /// Returns the complete Room Head captured for this Session.
    #[must_use]
    pub fn complete_head(&self) -> &CompleteHeadV1 {
        &self.complete_head
    }

    /// Returns the Membership frame head captured for this Session.
    #[must_use]
    pub const fn frame_head(&self) -> u64 {
        self.frame_head
    }

    /// Returns the retained-frame floor captured for this Session.
    #[must_use]
    pub const fn retained_floor(&self) -> u64 {
        self.retained_floor
    }

    /// Returns the durable Membership Cursor captured for this Session.
    #[must_use]
    pub const fn cursor(&self) -> Option<u64> {
        self.cursor
    }
}

/// One Session-bound barrier token. Its bytes are intentionally inaccessible.
#[derive(Clone)]
pub struct SessionSyncTokenV1([u8; 32]);

impl fmt::Debug for SessionSyncTokenV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionSyncTokenV1(REDACTED)")
    }
}

impl PartialEq for SessionSyncTokenV1 {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for SessionSyncTokenV1 {}

/// The exact barrier and token returned when attachment enters `CatchingUp`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CapturedSessionBarrierV1 {
    barrier: SessionBarrierV1,
    sync_token: SessionSyncTokenV1,
}

impl CapturedSessionBarrierV1 {
    /// Returns the captured positions.
    #[must_use]
    pub fn barrier(&self) -> SessionBarrierV1 {
        self.barrier.clone()
    }

    /// Returns the opaque token that only this Session may acknowledge.
    #[must_use]
    pub const fn sync_token(&self) -> &SessionSyncTokenV1 {
        &self.sync_token
    }
}

/// One storage-neutral addressed frame reference queued by a Session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionFrameV1 {
    frame_seq: u64,
}

impl SessionFrameV1 {
    /// Constructs one valid, positive Observation Frame sequence.
    ///
    /// # Errors
    ///
    /// Returns an error for sequence zero, which is not an Observation Frame.
    pub const fn new(frame_seq: u64) -> Result<Self, SessionErrorV1> {
        if frame_seq == 0 {
            return Err(SessionErrorV1::InvalidFrameSequence);
        }
        Ok(Self { frame_seq })
    }

    /// Returns the addressed frame sequence.
    #[must_use]
    pub const fn frame_seq(self) -> u64 {
        self.frame_seq
    }
}

/// Result of publishing a frame reference to a Session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionPublishOutcomeV1 {
    /// The frame was held until the attach barrier was acknowledged.
    Buffered,
    /// The Session is Live and the caller may deliver this frame immediately.
    DeliverNow(SessionFrameV1),
}

/// Closed failures and invalid operations on a Session.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SessionErrorV1 {
    /// A Session cannot have an empty post-barrier buffer.
    #[error("Session buffer capacity must be positive")]
    ZeroBufferCapacity,
    /// A present durable Cursor cannot be ahead of the captured frame head.
    #[error("Session Cursor is ahead of the captured frame head")]
    CursorAheadOfFrameHead,
    /// The retained floor is not a valid retained-range boundary.
    #[error("Session retained floor is invalid for the captured frame head")]
    InvalidRetainedFloor,
    /// Sequence zero is reserved for the no-frame Genesis position.
    #[error("Observation Frame sequence must be positive")]
    InvalidFrameSequence,
    /// The requested state transition is not available in the current state.
    #[error("Session operation is invalid in the current state")]
    InvalidState,
    /// A frame arrived that belongs to the captured range rather than the
    /// post-barrier queue.
    #[error("frame is not above the captured barrier")]
    FrameNotPostBarrier,
    /// A frame sequence is already queued for this Session.
    #[error("frame sequence is already buffered")]
    DuplicateBufferedFrame,
    /// A frame skipped the next sequence expected by this Session.
    #[error("frame sequence has a gap")]
    FrameSequenceGap,
    /// A frame sequence was older than the next sequence expected by this
    /// Session.
    #[error("frame sequence is stale")]
    FrameSequenceStale,
    /// The queue reached its configured bound; the Session is now Closed.
    #[error("Session closed for slow consumer")]
    SlowConsumer,
    /// The supplied token was not issued for this Session's barrier.
    #[error("Session synchronization token mismatch")]
    SyncTokenMismatch,
    /// The supplied baseline did not equal the captured frame head.
    #[error("Session synchronization baseline mismatch")]
    SyncBaselineMismatch,
    /// A matching synchronization acknowledgement was already consumed.
    #[error("Session synchronization acknowledgement was already consumed")]
    SyncAlreadyAcknowledged,
    /// The process-local Session nonce allocator reached its terminal value.
    #[error("Session synchronization token issuance is unavailable")]
    SessionNonceExhausted,
    /// The complete barrier could not be bound into an opaque token.
    #[error("Session synchronization token issuance is unavailable")]
    TokenIssuanceFailed,
}

/// Pure Session state machine for gap-free attach delivery.
pub struct SessionV1 {
    state: SessionStateV1,
    close_reason: Option<SessionCloseReasonV1>,
    buffer_capacity: usize,
    barrier: Option<CapturedSessionBarrierV1>,
    buffered_frames: BTreeMap<u64, SessionFrameV1>,
    next_frame_seq: Option<u64>,
    session_nonce: u64,
}

impl fmt::Debug for SessionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SessionV1")
            .field("state", &self.state)
            .field("close_reason", &self.close_reason)
            .field("buffer_capacity", &self.buffer_capacity)
            .field("barrier_captured", &self.barrier.is_some())
            .field("buffered_frame_count", &self.buffered_frames.len())
            .finish_non_exhaustive()
    }
}

impl SessionV1 {
    /// Creates an unattached Session with a bounded post-barrier buffer.
    ///
    /// # Errors
    ///
    /// Returns an error when the buffer capacity is zero.
    pub fn new(buffer_capacity: usize) -> Result<Self, SessionErrorV1> {
        if buffer_capacity == 0 {
            return Err(SessionErrorV1::ZeroBufferCapacity);
        }
        Ok(Self {
            state: SessionStateV1::Attaching,
            close_reason: None,
            buffer_capacity,
            barrier: None,
            buffered_frames: BTreeMap::new(),
            next_frame_seq: None,
            session_nonce: next_session_nonce()?,
        })
    }

    /// Returns the current delivery state.
    #[must_use]
    pub const fn state(&self) -> SessionStateV1 {
        self.state
    }

    /// Returns the operational close reason, if this Session is Closed.
    #[must_use]
    pub const fn close_reason(&self) -> Option<SessionCloseReasonV1> {
        self.close_reason
    }

    /// Returns the captured barrier, if attachment has entered `CatchingUp`.
    #[must_use]
    pub fn barrier(&self) -> Option<SessionBarrierV1> {
        self.barrier
            .as_ref()
            .map(|captured| captured.barrier.clone())
    }

    /// Captures one complete Room/frame barrier and issues its Session token.
    ///
    /// # Errors
    ///
    /// Returns [`SessionErrorV1::InvalidState`] unless the Session is still
    /// `Attaching`.
    pub fn capture_barrier(
        &mut self,
        barrier: SessionBarrierV1,
    ) -> Result<CapturedSessionBarrierV1, SessionErrorV1> {
        if self.state != SessionStateV1::Attaching {
            return Err(SessionErrorV1::InvalidState);
        }
        let token = SessionSyncTokenV1::issue(self.session_nonce, &barrier)?;
        let captured = CapturedSessionBarrierV1 {
            barrier: barrier.clone(),
            sync_token: token.clone(),
        };
        self.barrier = Some(captured);
        self.next_frame_seq = barrier.frame_head.checked_add(1);
        self.state = SessionStateV1::CatchingUp;
        Ok(CapturedSessionBarrierV1 {
            barrier,
            sync_token: token,
        })
    }

    /// Publishes a frame reference, buffering it until the barrier is `ACK`ed.
    ///
    /// # Errors
    ///
    /// Returns an error if the Session is not accepting delivery, if the frame
    /// is not above the captured barrier, if its sequence is already buffered,
    /// or if accepting it would exceed the bound (which closes the Session).
    pub fn publish(
        &mut self,
        frame: SessionFrameV1,
    ) -> Result<SessionPublishOutcomeV1, SessionErrorV1> {
        match self.state {
            SessionStateV1::Attaching | SessionStateV1::Closed => Err(SessionErrorV1::InvalidState),
            SessionStateV1::Live => {
                self.validate_next_frame(frame.frame_seq())?;
                self.advance_next_frame();
                Ok(SessionPublishOutcomeV1::DeliverNow(frame))
            }
            SessionStateV1::CatchingUp => {
                let barrier = self
                    .barrier
                    .as_ref()
                    .map(|captured| captured.barrier.clone())
                    .ok_or(SessionErrorV1::InvalidState)?;
                if frame.frame_seq <= barrier.frame_head {
                    return Err(SessionErrorV1::FrameNotPostBarrier);
                }
                if self.buffered_frames.contains_key(&frame.frame_seq) {
                    return Err(SessionErrorV1::DuplicateBufferedFrame);
                }
                self.validate_next_frame(frame.frame_seq())?;
                if self.buffered_frames.len() == self.buffer_capacity {
                    self.close_for(SessionCloseReasonV1::SlowConsumer);
                    return Err(SessionErrorV1::SlowConsumer);
                }
                self.buffered_frames.insert(frame.frame_seq, frame);
                self.advance_next_frame();
                Ok(SessionPublishOutcomeV1::Buffered)
            }
        }
    }

    /// Acknowledges exactly this Session's captured baseline and flushes its
    /// buffered post-barrier frames in ascending frame sequence order.
    ///
    /// # Errors
    ///
    /// Returns an error for a closed or not-yet-captured Session, a token or
    /// baseline mismatch, or an acknowledgement that was already consumed.
    pub fn sync_ack(
        &mut self,
        sync_token: &SessionSyncTokenV1,
        through_frame_head: u64,
    ) -> Result<Vec<SessionFrameV1>, SessionErrorV1> {
        if self.state == SessionStateV1::Live {
            return Err(SessionErrorV1::SyncAlreadyAcknowledged);
        }
        if self.state != SessionStateV1::CatchingUp {
            return Err(SessionErrorV1::InvalidState);
        }
        let captured = self.barrier.as_ref().ok_or(SessionErrorV1::InvalidState)?;
        if captured.sync_token != *sync_token {
            return Err(SessionErrorV1::SyncTokenMismatch);
        }
        if through_frame_head != captured.barrier.frame_head {
            return Err(SessionErrorV1::SyncBaselineMismatch);
        }
        self.state = SessionStateV1::Live;
        Ok(std::mem::take(&mut self.buffered_frames)
            .into_values()
            .collect())
    }

    /// Closes the Session when visibility loss makes incremental delivery
    /// unsafe.
    ///
    /// # Errors
    ///
    /// Returns [`SessionErrorV1::InvalidState`] if the Session is already
    /// closed.
    pub fn close_for_visibility_loss(&mut self) -> Result<(), SessionErrorV1> {
        if self.state == SessionStateV1::Closed {
            return Err(SessionErrorV1::InvalidState);
        }
        self.close_for(SessionCloseReasonV1::VisibilityLoss);
        Ok(())
    }

    fn close_for(&mut self, reason: SessionCloseReasonV1) {
        self.state = SessionStateV1::Closed;
        self.close_reason = Some(reason);
        self.barrier = None;
        self.buffered_frames.clear();
        self.next_frame_seq = None;
    }

    fn validate_next_frame(&self, frame_seq: u64) -> Result<(), SessionErrorV1> {
        let Some(next_frame_seq) = self.next_frame_seq else {
            return Err(SessionErrorV1::FrameSequenceStale);
        };
        if frame_seq < next_frame_seq {
            return Err(SessionErrorV1::FrameSequenceStale);
        }
        if frame_seq > next_frame_seq {
            return Err(SessionErrorV1::FrameSequenceGap);
        }
        Ok(())
    }

    fn advance_next_frame(&mut self) {
        self.next_frame_seq = self.next_frame_seq.and_then(|next| next.checked_add(1));
    }
}

impl SessionSyncTokenV1 {
    fn issue(session_nonce: u64, barrier: &SessionBarrierV1) -> Result<Self, SessionErrorV1> {
        let head_bytes = barrier
            .complete_head
            .canonical_bytes()
            .map_err(|_| SessionErrorV1::TokenIssuanceFailed)?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"worldstream/session-sync-token/v1");
        hasher.update(&session_nonce.to_le_bytes());
        hasher.update(&head_bytes);
        hasher.update(&barrier.frame_head.to_le_bytes());
        hasher.update(&barrier.retained_floor.to_le_bytes());
        match barrier.cursor {
            Some(cursor) => {
                hasher.update(&[1]);
                hasher.update(&cursor.to_le_bytes());
            }
            None => {
                hasher.update(&[0]);
            }
        }
        Ok(Self(*hasher.finalize().as_bytes()))
    }
}

fn next_session_nonce() -> Result<u64, SessionErrorV1> {
    NEXT_SESSION_NONCE
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            current.checked_add(1)
        })
        .map_err(|_| SessionErrorV1::SessionNonceExhausted)
}

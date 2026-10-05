#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{
    LiveStreamRegistry, OutboundFrameReservation, OutboundPayloadReservation,
    publication::PublicationBudget,
};

struct DeadlineBackend {
    entered: Arc<AtomicUsize>,
    release: Arc<AtomicBool>,
}
impl crate::GatewayBackend for DeadlineBackend {
    fn admission_principal(&self, _: &GatewaySession) -> Result<String, crate::BackendError> {
        Err(crate::BackendError::StorageUnavailable)
    }
    fn hello(
        &self,
        _: &GatewaySession,
        _: &worldstream_protocol::ClientHello,
    ) -> Result<worldstream_protocol::ServerWelcome, crate::BackendError> {
        Err(crate::BackendError::StorageUnavailable)
    }
    fn create_room(
        &self,
        _: &GatewaySession,
        _: worldstream_protocol::CreateRoomRequest,
    ) -> Result<worldstream_protocol::CreateRoomResponse, crate::BackendError> {
        Err(crate::BackendError::StorageUnavailable)
    }
    fn projection(
        &self,
        _: &GatewaySession,
        _: &str,
    ) -> Result<worldstream_protocol::ProjectionResponse, crate::BackendError> {
        Err(crate::BackendError::StorageUnavailable)
    }
    fn attach(
        &self,
        _: &GatewaySession,
        _: worldstream_protocol::RoomAttach,
    ) -> Result<crate::AttachReply, crate::BackendError> {
        Err(crate::BackendError::StorageUnavailable)
    }
    fn sync_ack(
        &self,
        _: &GatewaySession,
        _: worldstream_protocol::RoomSyncAck,
    ) -> Result<Vec<worldstream_protocol::ObservationDeliver>, crate::BackendError> {
        Err(crate::BackendError::StorageUnavailable)
    }
    fn observation_ack(
        &self,
        _: &GatewaySession,
        _: worldstream_protocol::ObservationAck,
    ) -> Result<Option<u64>, crate::BackendError> {
        panic!("publication must not acknowledge Cursor")
    }
    fn action(
        &self,
        _: &GatewaySession,
        _: worldstream_protocol::ActionSubmit,
    ) -> Result<crate::ActionReply, crate::BackendError> {
        Err(crate::BackendError::StorageUnavailable)
    }
    fn live_observation_suffix(
        &self,
        _: &GatewaySession,
        room: &str,
        member: &str,
        after: u64,
    ) -> Result<Vec<worldstream_protocol::ObservationDeliver>, crate::BackendError> {
        if room != "healthy" {
            self.entered.fetch_add(1, Ordering::Release);
            while !self.release.load(Ordering::Acquire) {
                if worldstream_core::storage_read_deadline()
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    return Err(crate::BackendError::StorageUnavailable);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            return Err(crate::BackendError::StorageUnavailable);
        }
        if after >= 1 {
            return Ok(Vec::new());
        }
        Ok(vec![worldstream_protocol::ObservationDeliver {
            room_id: room.into(),
            member_id: member.into(),
            frame_seq: 1,
            cause_room_seq: 1,
            frame_kind: "transition".into(),
            observation_schema: "test/v1".into(),
            observation: serde_json::json!({"healthy": true}),
            frame_payload_hash: "hash".into(),
        }])
    }
}

#[tokio::test]
async fn three_blocked_rooms_release_preparation_for_a_healthy_room() {
    // Each real scheduled batch reserves 9 MiB. Three retain 27 of 32 MiB.
    let entered = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(AtomicBool::new(false));
    struct ReleaseOnDrop(Arc<AtomicBool>);
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let _release_on_drop = ReleaseOnDrop(release.clone());
    let backend = Arc::new(DeadlineBackend {
        entered: entered.clone(),
        release: release.clone(),
    });
    let registry = LiveStreamRegistry::default();
    let baseline = registry.publication_budget.available_capacity();
    let mut receivers = Vec::new();
    let mut closes = Vec::new();
    for index in 0..4 {
        let session = Arc::new(GatewaySession::new(
            format!("01ARZ3NDEKTSV4RRFFQ69G5FF{index}").parse().unwrap(),
            CapabilityBearerV1::from_bytes([0x5a; 32]),
        ));
        let room = if index == 3 {
            "healthy".to_owned()
        } else {
            format!("blocked-{index}")
        };
        let (sender, receiver) = tokio::sync::mpsc::channel(crate::LIVE_PUSH_CAPACITY);
        let (close, close_receiver) = tokio::sync::watch::channel(None);
        registry
            .register(session, room.clone(), "member".into(), 0, sender, close)
            .unwrap();
        receivers.push(receiver);
        closes.push(close_receiver);
        if index < 3 {
            crate::schedule_live_publication(backend.clone(), &registry, &room);
        }
    }
    tokio::time::timeout(Duration::from_millis(500), async {
        while entered.load(Ordering::Acquire) < 3 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        registry.publication_budget.available_capacity().0,
        baseline.0 - 3
    );
    crate::schedule_live_publication(backend.clone(), &registry, "healthy");
    let healthy = tokio::time::timeout(Duration::from_secs(2), receivers[3].recv()).await;
    // Release the broken backend before asserting, so a red test cannot hang shutdown.
    release.store(true, Ordering::Release);
    assert!(
        healthy.is_ok_and(|push| push.is_some()),
        "three blocked Room reads starved healthy Room publication"
    );
    drop(registry);
    drop(receivers);
    tokio::time::timeout(Duration::from_secs(1), async {
        while PublicationBudget::default().available_capacity() != baseline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn publication_last_owner_drop_cancels_workers_but_retains_blocking_reservation() {
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let backend = Arc::new(BlockedRoomPublicationBackend {
        room_a_entered: entered.clone(),
        release_room_a: release.clone(),
        scheduler_tick_calls: Arc::new(AtomicUsize::new(0)),
        scheduler_tick_blocked: Arc::new(AtomicBool::new(false)),
        release_scheduler_tick: None,
    });
    let backend_weak = Arc::downgrade(&backend);
    let budget = PublicationBudget::default();
    let baseline = budget.available_capacity();
    let registry = LiveStreamRegistry::default();
    let weak_registry = Arc::downgrade(&registry.inner);
    let session = Arc::new(GatewaySession::new(
        "01ARZ3NDEKTSV4RRFFQ69G5FF0".parse().unwrap(),
        CapabilityBearerV1::from_bytes([0x5a; 32]),
    ));
    let (sender, receiver) = tokio::sync::mpsc::channel(crate::LIVE_PUSH_CAPACITY);
    let (close, _) = tokio::sync::watch::channel(None);
    registry
        .register(session, "room-a".into(), "member".into(), 0, sender, close)
        .unwrap();
    crate::schedule_live_publication(backend.clone(), &registry, "room-a");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !entered.load(Ordering::Acquire) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    assert!(budget.available_capacity().0 < baseline.0);
    drop(backend);
    drop(registry);
    drop(receiver);
    tokio::time::timeout(Duration::from_millis(250), async {
        while weak_registry.upgrade().is_some() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
    // Cancellation cannot claim that an active synchronous read has ended.
    assert!(backend_weak.upgrade().is_some());
    assert!(budget.available_capacity().0 < baseline.0);
    release.store(true, Ordering::Release);
    tokio::time::timeout(Duration::from_secs(1), async {
        while backend_weak.upgrade().is_some() || budget.available_capacity() != baseline {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .unwrap();
}

#[test]
fn publication_process_queue_limits_release_all_reservations() {
    let metrics = crate::LiveStreamQueueMetrics::default();
    let mut bytes = Vec::new();
    for _ in 0..8 {
        bytes.push(
            OutboundPayloadReservation::try_new(
                Arc::new(AtomicUsize::new(0)),
                crate::MAX_OUTBOUND_BUFFER_BYTES,
                &metrics.payload_bytes,
            )
            .unwrap(),
        );
    }
    assert_eq!(
        metrics
            .payload_bytes
            .snapshot("bytes", crate::MAX_OUTBOUND_BUFFER_BYTES)
            .process_current,
        crate::publication::PROCESS_QUEUE_BYTES
    );
    assert!(
        OutboundPayloadReservation::try_new(
            Arc::new(AtomicUsize::new(0)),
            1,
            &metrics.payload_bytes
        )
        .is_none()
    );
    drop(bytes);
    assert_eq!(
        metrics
            .payload_bytes
            .snapshot("bytes", crate::MAX_OUTBOUND_BUFFER_BYTES)
            .process_current,
        0
    );
    let mut frames = Vec::new();
    for _ in 0..8 {
        let unit = Arc::new(AtomicUsize::new(0));
        for _ in 0..crate::LIVE_PUSH_CAPACITY {
            frames.push(OutboundFrameReservation::try_new(unit.clone(), &metrics.frames).unwrap());
        }
    }
    assert_eq!(
        metrics
            .frames
            .snapshot("frames", crate::LIVE_PUSH_CAPACITY)
            .process_current,
        crate::publication::PROCESS_QUEUE_FRAMES
    );
    assert!(
        OutboundFrameReservation::try_new(Arc::new(AtomicUsize::new(0)), &metrics.frames).is_none()
    );
    drop(frames);
    assert_eq!(
        metrics
            .frames
            .snapshot("frames", crate::LIVE_PUSH_CAPACITY)
            .process_current,
        0
    );
}

#[test]
fn publication_preparation_capacity_is_shared_across_registries() {
    let first = LiveStreamRegistry::default();
    let second = LiveStreamRegistry::default();
    let baseline = first.publication_budget.available_capacity();
    let mut reservations = Vec::new();
    while let Some(reservation) = first.publication_budget.reserve_preparation() {
        reservations.push(reservation);
    }
    assert!(second.publication_budget.reserve_preparation().is_none());
    drop(reservations);
    assert_eq!(first.publication_budget.available_capacity(), baseline);
    assert_eq!(second.publication_budget.available_capacity(), baseline);
}

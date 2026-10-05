//! Bounded operational wake-ups. Durable frames and Cursor remain backend facts.
use super::*;
use std::collections::VecDeque;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

pub(super) const MAX_REGISTERED_SESSIONS: usize = 4096;
const WORKERS: usize = 4;
const MAX_PENDING_ROOMS: usize = 64;
const MAX_BLOCKING_READS: usize = 8;
const SCAN_ROOMS: usize = 16;
const SLICE_SESSIONS: usize = 16;
const PAGE_FRAMES: usize = 32;
const PAGE_CANONICAL_BYTES: usize = 1024 * 1024;
pub(super) const PROCESS_QUEUE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const PROCESS_QUEUE_FRAMES: usize = 2048;
const PROCESS_PREPARATION_BYTES: usize = 32 * 1024 * 1024;
// Covers the compatibility reader's existing 4-MiB canonical allowance and
// one serialized wire envelope. These are logical bytes, not an RSS estimate.
const PREPARATION_RESERVATION_BYTES: usize =
    4 * 1024 * 1024 + worldstream_protocol::MAX_MESSAGE_BYTES;
const READ_DEADLINE: Duration = Duration::from_secs(1);
// Leave time for SQL cancellation and guard release before the caller gives up.
const STORAGE_READ_BUDGET: Duration = Duration::from_millis(750);

// Cancel a queued blocking closure when its async caller times out or is
// canceled. Running native work keeps its own reservations until it exits.
struct AbortQueuedRead<T>(tokio::task::JoinHandle<T>);
impl<T> Drop for AbortQueuedRead<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}
const SLICE_DEADLINE: Duration = Duration::from_secs(2);
const REDISCOVERY_INTERVAL: Duration = Duration::from_millis(250);

#[cfg(test)]
pub(super) static PREPARATION_RESERVATIONS: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
pub(super) static PREPARATION_HIGH_WATER_BYTES: AtomicUsize = AtomicUsize::new(0);
pub(super) struct PublicationLifetime {
    stop: watch::Sender<bool>,
}
impl Default for PublicationLifetime {
    fn default() -> Self {
        let (stop, _) = watch::channel(false);
        Self { stop }
    }
}
impl Drop for PublicationLifetime {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

#[derive(Clone)]
pub(super) struct PublicationBudget {
    reads: Arc<Semaphore>,
    preparation: Arc<Semaphore>,
    in_flight_rooms: Arc<Mutex<HashSet<String>>>,
}
impl Default for PublicationBudget {
    fn default() -> Self {
        static PROCESS_PERMITS: OnceLock<(Arc<Semaphore>, Arc<Semaphore>)> = OnceLock::new();
        let (reads, preparation) = PROCESS_PERMITS.get_or_init(|| {
            (
                Arc::new(Semaphore::new(MAX_BLOCKING_READS)),
                Arc::new(Semaphore::new(PROCESS_PREPARATION_BYTES)),
            )
        });
        Self {
            reads: Arc::clone(reads),
            preparation: Arc::clone(preparation),
            in_flight_rooms: Arc::new(Mutex::new(HashSet::new())),
        }
    }
}

struct ReadReservation {
    _read: OwnedSemaphorePermit,
    _preparation: OwnedSemaphorePermit,
    _extra_preparation: Option<OwnedSemaphorePermit>,
    _room: RoomReadFence,
}
struct RoomReadFence {
    rooms: Arc<Mutex<HashSet<String>>>,
    room_id: String,
}
impl Drop for RoomReadFence {
    fn drop(&mut self) {
        self.rooms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.room_id);
    }
}
impl PublicationBudget {
    #[cfg(test)]
    pub(super) fn available_capacity(&self) -> (usize, usize) {
        (
            self.reads.available_permits(),
            self.preparation.available_permits(),
        )
    }
    pub(super) fn reserve_preparation(&self) -> Option<OwnedSemaphorePermit> {
        let permit = Arc::clone(&self.preparation)
            .try_acquire_many_owned(u32::try_from(PREPARATION_RESERVATION_BYTES).ok()?)
            .ok()?;
        #[cfg(test)]
        {
            PREPARATION_RESERVATIONS.fetch_add(1, Ordering::Relaxed);
            atomic_max_usize(
                &PREPARATION_HIGH_WATER_BYTES,
                PROCESS_PREPARATION_BYTES - self.preparation.available_permits(),
            );
        }
        Some(permit)
    }
    fn reserve_batch(&self, room_id: &str) -> Option<ReadReservation> {
        let mut reservation = self.reserve(room_id)?;
        let second = self.reserve_preparation()?;
        reservation._extra_preparation = Some(second);
        Some(reservation)
    }
    fn reserve(&self, room_id: &str) -> Option<ReadReservation> {
        let read = Arc::clone(&self.reads).try_acquire_owned().ok()?;
        let preparation = self.reserve_preparation()?;
        if !self
            .in_flight_rooms
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(room_id.to_owned())
        {
            return None;
        }
        Some(ReadReservation {
            _read: read,
            _preparation: preparation,
            _extra_preparation: None,
            _room: RoomReadFence {
                rooms: Arc::clone(&self.in_flight_rooms),
                room_id: room_id.to_owned(),
            },
        })
    }
}

#[derive(Clone)]
struct RoomWork {
    room_id: String,
    after_session_id: Option<String>,
    more_frames: bool,
}
#[derive(Default)]
struct Pending {
    queue: VecDeque<RoomWork>,
    queued: HashSet<String>,
    running: HashMap<String, bool>,
    overflow: bool,
    scan_after: Option<String>,
}
struct PublicationState {
    pending: Mutex<Pending>,
    notify: Notify,
}
pub(super) struct PublicationCoordinator {
    state: Arc<PublicationState>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for PublicationCoordinator {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl PublicationCoordinator {
    fn start(registry: &LiveStreamRegistry, backend: Arc<dyn GatewayBackend>) -> Self {
        let state = Arc::new(PublicationState {
            pending: Mutex::new(Pending::default()),
            notify: Notify::new(),
        });
        let mut tasks = Vec::with_capacity(WORKERS + 1);
        for _ in 0..WORKERS {
            tasks.push(tokio::spawn(worker(
                Arc::downgrade(&registry.inner),
                Arc::clone(&backend),
                Arc::clone(&state),
                registry.lifetime.stop.subscribe(),
            )));
        }
        tasks.push(tokio::spawn(rediscover(
            Arc::downgrade(&registry.inner),
            Arc::clone(&state),
            registry.lifetime.stop.subscribe(),
        )));
        Self { state, tasks }
    }
    fn schedule(&self, room_id: &str) {
        let mut pending = self
            .state
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        schedule(&mut pending, room_id);
        drop(pending);
        self.state.notify.notify_waiters();
    }
}
fn schedule(pending: &mut Pending, room_id: &str) {
    if let Some(dirty) = pending.running.get_mut(room_id) {
        *dirty = true;
        return;
    }
    if pending.queued.contains(room_id) {
        return;
    }
    if pending.queue.len() >= MAX_PENDING_ROOMS {
        pending.overflow = true;
        return;
    }
    pending.queued.insert(room_id.to_owned());
    pending.queue.push_back(RoomWork {
        room_id: room_id.to_owned(),
        after_session_id: None,
        more_frames: false,
    });
}
impl LiveStreamRegistry {
    pub(super) fn ensure_publication(&self, backend: Arc<dyn GatewayBackend>) {
        let mut coordinator = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if coordinator
            .as_ref()
            .is_none_or(|owner| owner.tasks.iter().all(tokio::task::JoinHandle::is_finished))
        {
            *coordinator = Some(PublicationCoordinator::start(self, backend));
        }
    }
    fn publication_snapshots(
        &self,
        room_id: &str,
        after: Option<&str>,
    ) -> (Vec<LiveStreamSnapshot>, Option<String>) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(ids) = state.rooms.get(room_id) else {
            return (Vec::new(), None);
        };
        let ids: Vec<&String> = ids
            .iter()
            .filter(|id| after.is_none_or(|after| id.as_str() > after))
            .take(SLICE_SESSIONS + 1)
            .collect();
        let next = (ids.len() > SLICE_SESSIONS).then(|| ids[SLICE_SESSIONS - 1].to_string());
        let snapshots = ids
            .into_iter()
            .take(SLICE_SESSIONS)
            .filter_map(|id| state.sessions.get(id))
            .map(|registration| LiveStreamSnapshot {
                session_id: registration.session.session_id().to_string(),
                session: Arc::clone(&registration.session),
                room_id: registration.room_id.clone(),
                member_id: registration.member_id.clone(),
                generation: registration.generation,
                last_delivered_frame_seq: registration.last_delivered_frame_seq,
                sender: registration.sender.clone(),
                queued_frames: Arc::clone(&registration.queued_frames),
                queued_payload_bytes: Arc::clone(&registration.queued_payload_bytes),
            })
            .collect();
        (snapshots, next)
    }
    fn enqueue_current(
        &self,
        snapshot: &LiveStreamSnapshot,
        push: LivePush,
        seq: u64,
    ) -> Result<(), ()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(registration) = state
            .sessions
            .get_mut(&snapshot.session_id)
            .filter(|r| r.generation == snapshot.generation && r.room_id == snapshot.room_id)
        else {
            return Ok(());
        };
        registration.sender.try_send(push).map_err(|_| ())?;
        registration.last_delivered_frame_seq = seq;
        Ok(())
    }
}
pub(super) fn schedule_live_publication(
    backend: Arc<dyn GatewayBackend>,
    registry: &LiveStreamRegistry,
    room_id: &str,
) {
    if !registry
        .state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .rooms
        .contains_key(room_id)
    {
        return;
    }
    registry.ensure_publication(backend);
    if let Some(coordinator) = registry
        .publication
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        coordinator.schedule(room_id);
    }
}
async fn rediscover(
    registry: Weak<LiveStreamRegistryInner>,
    state: Arc<PublicationState>,
    mut stop: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(REDISCOVERY_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! { _ = tick.tick() => {}, _ = stop.changed() => return }
        let Some(inner) = registry.upgrade() else {
            return;
        };
        let after = state
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .scan_after
            .clone();
        let (rooms, next) = {
            let registry = inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let rooms: Vec<String> = registry
                .rooms
                .range::<String, _>((
                    after
                        .as_ref()
                        .map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded),
                    std::ops::Bound::Unbounded,
                ))
                .map(|(room, _)| room.clone())
                .take(SCAN_ROOMS + 1)
                .collect();
            let next = (rooms.len() > SCAN_ROOMS).then(|| rooms[SCAN_ROOMS - 1].clone());
            (rooms.into_iter().take(SCAN_ROOMS).collect::<Vec<_>>(), next)
        };
        drop(inner);
        let mut pending = state
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Clear only after a complete index pass; each skipped bounded-queue
        // insertion sets the flag again. Periodic scans also heal missed wakes.
        if next.is_none() {
            pending.overflow = false;
        }
        pending.scan_after = next;
        for room in rooms {
            schedule(&mut pending, &room);
        }
        drop(pending);
        state.notify.notify_waiters();
    }
}
async fn worker(
    registry: Weak<LiveStreamRegistryInner>,
    backend: Arc<dyn GatewayBackend>,
    state: Arc<PublicationState>,
    mut stop: watch::Receiver<bool>,
) {
    loop {
        let notified = state.notify.notified();
        let work = {
            let mut pending = state
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let work = pending.queue.pop_front();
            if let Some(work) = &work {
                pending.queued.remove(&work.room_id);
                pending.running.insert(work.room_id.clone(), false);
            }
            work
        };
        let Some(mut work) = work else {
            tokio::select! { _ = notified => {}, _ = stop.changed() => return }
            continue;
        };
        let Some(inner) = registry.upgrade() else {
            return;
        };
        let live = LiveStreamRegistry {
            inner,
            lifetime: Arc::new(PublicationLifetime::default()),
        };
        let (next, more) = tokio::select! {
            result = tokio::time::timeout(SLICE_DEADLINE, publish_slice(Arc::clone(&backend), &live, &work)) => result.unwrap_or((None, true)),
            _ = stop.changed() => return,
        };
        drop(live);
        work.more_frames |= more;
        {
            let mut pending = state
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dirty = pending.running.remove(&work.room_id).unwrap_or(false);
            if next.is_some() || work.more_frames || dirty {
                work.after_session_id = next;
                if work.after_session_id.is_none() {
                    work.more_frames = false;
                }
                if pending.queue.len() < MAX_PENDING_ROOMS {
                    pending.queued.insert(work.room_id.clone());
                    pending.queue.push_back(work);
                } else {
                    pending.overflow = true;
                }
            }
        }
        state.notify.notify_waiters();
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn publish_slice(
    backend: Arc<dyn GatewayBackend>,
    registry: &LiveStreamRegistry,
    work: &RoomWork,
) -> (Option<String>, bool) {
    let room_lock = registry.room_publication_lock(&work.room_id);
    let _publication_guard = room_lock.lock().await;
    let (snapshots, mut next) =
        registry.publication_snapshots(&work.room_id, work.after_session_id.as_deref());
    let maximum = if backend.supports_shared_live_observation_cut() {
        SLICE_SESSIONS
    } else {
        2
    };
    let mut addresses = HashSet::new();
    let mut selected = Vec::new();
    for snapshot in snapshots {
        let address = (
            snapshot.member_id.clone(),
            snapshot.last_delivered_frame_seq,
        );
        if selected.len() == maximum || (!addresses.contains(&address) && addresses.len() == 2) {
            next = selected
                .last()
                .map(|snapshot: &LiveStreamSnapshot| snapshot.session_id.clone());
            break;
        }
        addresses.insert(address);
        selected.push(snapshot);
    }
    if selected.is_empty() {
        return (next, false);
    }
    let Some(reservation) = registry.publication_budget.reserve_batch(&work.room_id) else {
        return (next, true);
    };
    let recipients: Vec<_> = selected
        .iter()
        .map(|snapshot| LiveObservationRecipient {
            session: Arc::clone(&snapshot.session),
            room_id: snapshot.room_id.clone(),
            member_id: snapshot.member_id.clone(),
            after_frame_seq: snapshot.last_delivered_frame_seq,
        })
        .collect();
    let requests = recipients.clone();
    let reader = Arc::clone(&backend);
    let storage_deadline = std::time::Instant::now() + STORAGE_READ_BUDGET;
    let mut read = AbortQueuedRead(tokio::task::spawn_blocking(move || {
        (
            worldstream_core::with_storage_read_deadline(storage_deadline, || {
                reader.prepare_live_observation_batch(&requests, PAGE_FRAMES, PAGE_CANONICAL_BYTES)
            }),
            reservation,
        )
    }));
    let (pages, _reservation) = match tokio::time::timeout(READ_DEADLINE, &mut read.0).await {
        Ok(Ok(result)) => result,
        _ => return (next, true),
    };
    if pages.len() != selected.len() {
        return (next, true);
    }
    let unique: HashSet<_> = pages
        .iter()
        .filter_map(|page| page.as_ref().ok())
        .map(|page| Arc::as_ptr(&page.frames) as *const ObservationDeliver as usize)
        .collect();
    if unique.len() > 2 {
        for snapshot in &selected {
            registry.close_generation(
                &snapshot.session_id,
                snapshot.generation,
                ErrorCode::Internal,
            );
        }
        return (next, false);
    }
    let mut bodies: HashMap<usize, Vec<String>> = HashMap::new();
    let mut more = false;
    for ((snapshot, recipient), page) in selected.into_iter().zip(recipients).zip(pages) {
        let page = match page {
            Ok(page) => page,
            Err(error) => {
                registry.close_generation(&snapshot.session_id, snapshot.generation, error.code());
                continue;
            }
        };
        more |= page.has_more;
        let valid_page = page.frames.len() <= PAGE_FRAMES
            && page
                .frames
                .iter()
                .try_fold(0usize, |bytes, frame| {
                    let mut writer = CountingWireWriter(0);
                    serde_json::to_writer(&mut writer, frame).ok()?;
                    bytes.checked_add(writer.0)
                })
                .is_some_and(|bytes| bytes <= PAGE_CANONICAL_BYTES + PAGE_FRAMES * 1024);
        if !valid_page {
            registry.close_generation(
                &snapshot.session_id,
                snapshot.generation,
                ErrorCode::Internal,
            );
            continue;
        }
        let key = Arc::as_ptr(&page.frames) as *const ObservationDeliver as usize;
        if !bodies.contains_key(&key) {
            let encoded = page
                .frames
                .iter()
                .map(serialize_live_body)
                .collect::<Result<Vec<_>, _>>();
            match encoded {
                Ok(encoded) => {
                    bodies.insert(key, encoded);
                }
                Err(error) => {
                    registry.close_generation(
                        &snapshot.session_id,
                        snapshot.generation,
                        error.code(),
                    );
                    continue;
                }
            }
        }
        let mut expected = snapshot.last_delivered_frame_seq.saturating_add(1);
        for (frame, body) in page.frames.iter().zip(&bodies[&key]) {
            if frame.room_id != snapshot.room_id
                || frame.member_id != snapshot.member_id
                || frame.frame_seq != expected
            {
                registry.close_generation(
                    &snapshot.session_id,
                    snapshot.generation,
                    ErrorCode::Internal,
                );
                break;
            }
            if let Err(error) = validate_current_delivery(
                Arc::clone(&backend),
                registry.publication_budget.clone(),
                recipient.clone(),
                page.fence.clone(),
                frame.frame_seq,
                frame.cause_room_seq,
                frame.frame_payload_hash.clone(),
            )
            .await
            {
                registry.close_generation(&snapshot.session_id, snapshot.generation, error.code());
                break;
            }
            match prepare_shared_live_push(
                frame,
                body,
                snapshot.generation,
                recipient.clone(),
                page.fence.clone(),
                Arc::clone(&snapshot.queued_frames),
                Arc::clone(&snapshot.queued_payload_bytes),
                &registry.queue_metrics,
            ) {
                Ok(push) => {
                    if registry
                        .enqueue_current(&snapshot, push, frame.frame_seq)
                        .is_err()
                    {
                        registry.close_generation(
                            &snapshot.session_id,
                            snapshot.generation,
                            ErrorCode::SlowConsumer,
                        );
                        break;
                    }
                }
                Err(error) => {
                    registry.close_generation(
                        &snapshot.session_id,
                        snapshot.generation,
                        error.code(),
                    );
                    break;
                }
            }
            expected = frame.frame_seq.saturating_add(1);
        }
    }
    (next, more)
}

pub(super) async fn validate_current_delivery(
    backend: Arc<dyn GatewayBackend>,
    budget: PublicationBudget,
    recipient: LiveObservationRecipient,
    fence: Option<Arc<dyn LiveObservationFence>>,
    seq: u64,
    cause: u64,
    hash: String,
) -> Result<(), BackendError> {
    let read = tokio::time::timeout(READ_DEADLINE, Arc::clone(&budget.reads).acquire_owned())
        .await
        .map_err(|_| BackendError::StorageUnavailable)?
        .map_err(|_| BackendError::StorageUnavailable)?;
    let preparation = if fence.is_none() {
        Some(
            tokio::time::timeout(
                READ_DEADLINE,
                Arc::clone(&budget.preparation).acquire_many_owned(
                    u32::try_from(PREPARATION_RESERVATION_BYTES)
                        .map_err(|_| BackendError::InvalidResult)?,
                ),
            )
            .await
            .map_err(|_| BackendError::StorageUnavailable)?
            .map_err(|_| BackendError::StorageUnavailable)?,
        )
    } else {
        None
    };
    let storage_deadline = std::time::Instant::now() + STORAGE_READ_BUDGET;
    let mut check = AbortQueuedRead(tokio::task::spawn_blocking(move || {
        let _read = read;
        let _preparation = preparation;
        worldstream_core::with_storage_read_deadline(storage_deadline, || {
            if let Some(fence) = fence {
                fence.revalidate(&recipient.session)
            } else {
                let page = backend.live_observation_page(
                    &recipient.session,
                    &recipient.room_id,
                    &recipient.member_id,
                    seq.saturating_sub(1),
                    1,
                    PAGE_CANONICAL_BYTES,
                )?;
                if page.frames.first().is_some_and(|frame| {
                    frame.frame_seq == seq
                        && frame.cause_room_seq == cause
                        && frame.frame_payload_hash == hash
                        && frame.room_id == recipient.room_id
                        && frame.member_id == recipient.member_id
                }) {
                    Ok(())
                } else {
                    Err(BackendError::InvalidResult)
                }
            }
        })
    }));
    tokio::time::timeout(READ_DEADLINE, &mut check.0)
        .await
        .map_err(|_| BackendError::StorageUnavailable)?
        .map_err(|_| BackendError::StorageUnavailable)?
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod queued_read_tests {
    use super::AbortQueuedRead;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        time::Duration,
    };

    #[test]
    fn publication_cancellation_aborts_a_queued_read_and_releases_its_guard() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let busy = runtime.spawn_blocking(move || {
            entered.send(()).unwrap();
            // Bounded cleanup also makes a red assertion safe for shutdown.
            let _ = released.recv_timeout(Duration::from_secs(2));
        });
        entry.recv_timeout(Duration::from_secs(1)).unwrap();
        let ran = Arc::new(AtomicBool::new(false));
        let queued_ran = Arc::clone(&ran);
        let budget = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = runtime
            .block_on(Arc::clone(&budget).acquire_owned())
            .unwrap();
        let read = AbortQueuedRead(runtime.spawn_blocking(move || {
            let _permit = permit;
            queued_ran.store(true, Ordering::Release);
        }));
        assert_eq!(budget.available_permits(), 0);
        drop(read);
        release.send(()).unwrap();
        runtime.block_on(busy).unwrap();
        runtime.shutdown_timeout(Duration::from_secs(1));
        assert!(!ran.load(Ordering::Acquire));
        assert_eq!(budget.available_permits(), 1);
    }
}

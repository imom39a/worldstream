//! Bounded ownership of current Room executors between adapter operations.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use crate::{CoreTraceV1, RoomId, RoomIntegrityStateV1};

/// A recovered executor and the operational integrity fence it was recovered at.
/// Adapters must compare its complete Head and integrity with storage before use.
/// Historical Transitions and administrative receipt copies are discarded;
/// adapters must resolve operation identities against durable storage first.
/// This value deliberately cannot be cloned.
pub struct CachedRoomTraceV1 {
    trace: CoreTraceV1,
    integrity: RoomIntegrityStateV1,
}

impl CachedRoomTraceV1 {
    /// Retains a uniquely owned executor following successful fenced recovery.
    #[must_use]
    pub fn new(mut trace: CoreTraceV1, integrity: RoomIntegrityStateV1) -> Self {
        trace.discard_persisted_history();
        Self { trace, integrity }
    }

    /// Returns the current execution basis, including its complete Head.
    #[must_use]
    pub const fn trace(&self) -> &CoreTraceV1 {
        &self.trace
    }

    /// Borrows the executor exclusively for a prepared, storage-fenced commit.
    pub const fn trace_mut(&mut self) -> &mut CoreTraceV1 {
        &mut self.trace
    }

    /// Returns the operational integrity state established by recovery.
    #[must_use]
    pub const fn integrity(&self) -> &RoomIntegrityStateV1 {
        &self.integrity
    }

    /// Consumes the retained entry and transfers its unique executor ownership.
    #[must_use]
    pub fn into_trace(self) -> CoreTraceV1 {
        self.trace
    }
}

/// Failures to acquire bounded current-Room executor ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RoomTraceCacheErrorV1 {
    /// A cache must have room for at least one executor.
    #[error("Room trace cache capacity must be positive")]
    InvalidCapacity,
    /// Every retained Room is currently borrowed or has a waiting borrower.
    #[error("Room trace cache is busy")]
    Busy,
    /// An earlier callback panicked; a possibly damaged executor is never reused.
    #[error("Room trace cache lock is poisoned")]
    Poisoned,
}

type RoomSlot = Arc<Mutex<Option<CachedRoomTraceV1>>>;

struct CacheEntry {
    slot: RoomSlot,
    touched: u64,
}

#[derive(Default)]
struct CacheState {
    entries: BTreeMap<RoomId, CacheEntry>,
    clock: u64,
}

/// Keeps a bounded number of unique Room executors, serializing only borrowers
/// of the same Room. Least recently used idle entries may be evicted. Active
/// entries and entries with waiters cannot be evicted into a second lane.
///
/// Storage remains authoritative: an adapter must validate the complete Head,
/// integrity, current authority, and delivery positions on each operation. A
/// mismatch invalidates the entry and requires fenced recovery. Failed or
/// indeterminate commits must not install an in-memory advance.
pub struct RoomTraceCacheV1 {
    capacity: usize,
    state: Mutex<CacheState>,
}

impl Default for RoomTraceCacheV1 {
    fn default() -> Self {
        Self {
            capacity: 128,
            state: Mutex::new(CacheState::default()),
        }
    }
}

impl RoomTraceCacheV1 {
    /// Constructs a cache with a maximum number of resident Room slots.
    ///
    /// # Errors
    /// Returns [`RoomTraceCacheErrorV1::InvalidCapacity`] for zero capacity.
    pub fn new(capacity: usize) -> Result<Self, RoomTraceCacheErrorV1> {
        if capacity == 0 {
            return Err(RoomTraceCacheErrorV1::InvalidCapacity);
        }
        Ok(Self {
            capacity,
            state: Mutex::new(CacheState::default()),
        })
    }

    /// Borrows one Room's slot exclusively throughout `operation`. An empty
    /// slot needs recovery. Set it to `None` to discard a stale execution basis.
    /// The callback must not recursively acquire this cache or move an executor
    /// into another execution lane outside this ownership scope.
    ///
    /// # Errors
    /// Returns `Busy` if no idle slot can be evicted, or `Poisoned` after a panic.
    pub fn with_room<R>(
        &self,
        room_id: &RoomId,
        operation: impl FnOnce(&mut Option<CachedRoomTraceV1>) -> R,
    ) -> Result<R, RoomTraceCacheErrorV1> {
        let slot = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| RoomTraceCacheErrorV1::Poisoned)?;
            state.clock = state.clock.saturating_add(1);
            let touched = state.clock;
            if let Some(entry) = state.entries.get_mut(room_id) {
                entry.touched = touched;
                Arc::clone(&entry.slot)
            } else {
                if state.entries.len() == self.capacity {
                    let victim = state
                        .entries
                        .iter()
                        .filter(|(_, entry)| Arc::strong_count(&entry.slot) == 1)
                        .min_by_key(|(_, entry)| entry.touched)
                        .map(|(key, _)| key.clone())
                        .ok_or(RoomTraceCacheErrorV1::Busy)?;
                    state.entries.remove(&victim);
                }
                let slot = Arc::new(Mutex::new(None));
                state.entries.insert(
                    room_id.clone(),
                    CacheEntry {
                        slot: Arc::clone(&slot),
                        touched,
                    },
                );
                slot
            }
        };
        let mut guard = slot.lock().map_err(|_| RoomTraceCacheErrorV1::Poisoned)?;
        let result = operation(&mut guard);
        if let Some(cached) = guard.as_mut() {
            cached.trace.discard_persisted_history();
        }
        Ok(result)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::{
        sync::{Barrier, mpsc},
        thread,
        time::Duration,
    };

    fn room(index: u8) -> RoomId {
        format!("01ARZ3NDEKTSV4RRFFQ69G5FA{index}").parse().unwrap()
    }

    #[test]
    fn independent_rooms_progress_while_one_executor_is_borrowed() {
        let cache = RoomTraceCacheV1::new(2).unwrap();
        thread::scope(|scope| {
            let ready = Arc::new(Barrier::new(2));
            let (release, wait) = mpsc::channel();
            let borrowed = Arc::clone(&ready);
            let cache_ref = &cache;
            scope.spawn(move || {
                cache_ref
                    .with_room(&room(0), |_| {
                        borrowed.wait();
                        wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    })
                    .unwrap()
            });
            ready.wait();
            assert_eq!(cache.with_room(&room(1), |_| 42), Ok(42));
            release.send(()).unwrap();
        });
    }

    #[test]
    fn active_room_cannot_be_evicted_into_a_second_execution_lane() {
        let cache = RoomTraceCacheV1::new(1).unwrap();
        thread::scope(|scope| {
            let ready = Arc::new(Barrier::new(2));
            let (release, wait) = mpsc::channel();
            let borrowed = Arc::clone(&ready);
            let cache_ref = &cache;
            scope.spawn(move || {
                cache_ref
                    .with_room(&room(0), |_| {
                        borrowed.wait();
                        wait.recv_timeout(Duration::from_secs(5)).unwrap();
                    })
                    .unwrap()
            });
            ready.wait();
            assert_eq!(
                cache.with_room(&room(1), |_| ()),
                Err(RoomTraceCacheErrorV1::Busy)
            );
            release.send(()).unwrap();
        });
        assert_eq!(cache.with_room(&room(1), |_| 42), Ok(42));
    }

    #[test]
    fn same_room_borrowers_never_overlap() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let cache = RoomTraceCacheV1::new(1).unwrap();
        let active = AtomicUsize::new(0);
        let arrived = Barrier::new(8);
        thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    arrived.wait();
                    for _ in 0..100 {
                        cache
                            .with_room(&room(0), |_| {
                                assert_eq!(active.fetch_add(1, Ordering::SeqCst), 0);
                                thread::yield_now();
                                assert_eq!(active.fetch_sub(1, Ordering::SeqCst), 1);
                            })
                            .unwrap();
                    }
                });
            }
        });
    }

    #[test]
    fn callback_panic_fails_closed_without_poisoning_other_rooms() {
        let cache = RoomTraceCacheV1::new(2).unwrap();
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            cache.with_room(&room(0), |_| panic!("simulated executor failure"))
        }));
        assert!(failed.is_err());
        assert_eq!(
            cache.with_room(&room(0), |_| ()),
            Err(RoomTraceCacheErrorV1::Poisoned)
        );
        assert_eq!(cache.with_room(&room(1), |_| 42), Ok(42));
    }
}

//! Host-owned semantic time and bounded Room admission primitives.

#![cfg_attr(test, allow(clippy::panic, clippy::redundant_closure_for_method_calls))]

use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    sync::{
        Arc, Condvar, Mutex, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use crate::primitives::compare_timestamp_text;
use crate::{ActionAdmittedAt, RoomId, TimerScheduledFor};

/// Frozen production capacity of one per-Room admission lane.
pub const ROOM_ADMISSION_LANE_CAPACITY_V1: usize = 256;

/// Production positions unavailable to participant Actions so host stimuli
/// can still enter a participant-saturated Room lane.
pub const ROOM_ADMISSION_HOST_RESERVE_V1: usize = 16;

/// One normalized UTC sample supplied by the application-owned `HostClock`.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HostClockSampleV1(String);

impl HostClockSampleV1 {
    /// Parses one normalized UTC RFC 3339 sample.
    ///
    /// # Errors
    ///
    /// Returns the timestamp parser error when `value` is not canonical UTC
    /// time.
    pub fn new(value: impl AsRef<str>) -> Result<Self, crate::TimestampParseError> {
        let value = value.as_ref();
        value.parse::<ActionAdmittedAt>()?;
        Ok(Self(value.to_owned()))
    }

    /// Returns the normalized UTC sample text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Converts the sample to Action semantic time after lane admission.
    #[must_use]
    pub fn as_action_admitted_at(&self) -> ActionAdmittedAt {
        self.0
            .parse()
            .unwrap_or_else(|_| unreachable!("HostClockSampleV1 was validated at construction"))
    }

    /// Reports whether a timer is due at this sample, including equality.
    #[must_use]
    pub fn is_due(&self, timer: &TimerScheduledFor) -> bool {
        compare_timestamp_text(self.as_str(), timer.as_str()) != std::cmp::Ordering::Less
    }
}

/// Closed `HostClock` failures. Time-bearing work must fail closed on these
/// paths; callers must not synthesize a semantic timestamp.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum HostClockErrorV1 {
    /// The source could not provide a trusted sample.
    #[error("HostClock sample is unavailable")]
    Unavailable,
    /// The source moved backwards after a sample had been issued.
    #[error("HostClock sample moved backwards")]
    Rollback,
}

/// Application-owned source of normalized semantic host time.
pub trait HostClockV1: Send + Sync {
    /// Issues one sample. The source may be wrapped in
    /// [`MonotonicHostClockV1`] when it does not enforce monotonicity itself.
    ///
    /// # Errors
    ///
    /// Returns a typed failure when the source is unavailable or moves
    /// backwards.
    fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1>;
}

/// Monotonic fail-closed `HostClock` wrapper for an injectable source.
pub struct MonotonicHostClockV1<C> {
    source: C,
    last: Mutex<Option<HostClockSampleV1>>,
}

impl<C> fmt::Debug for MonotonicHostClockV1<C> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("MonotonicHostClockV1([OPAQUE])")
    }
}

impl<C> MonotonicHostClockV1<C> {
    /// Wraps an injectable source and starts with no issued sample.
    #[must_use]
    pub const fn new(source: C) -> Self {
        Self {
            source,
            last: Mutex::new(None),
        }
    }
}

impl<C: HostClockV1> HostClockV1 for MonotonicHostClockV1<C> {
    fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
        let sample = self.source.sample()?;
        let mut last = self
            .last
            .lock()
            .map_err(|_| HostClockErrorV1::Unavailable)?;
        if last.as_ref().is_some_and(|previous| {
            compare_timestamp_text(sample.as_str(), previous.as_str()) == std::cmp::Ordering::Less
        }) {
            return Err(HostClockErrorV1::Rollback);
        }
        *last = Some(sample.clone());
        Ok(sample)
    }
}

/// The source occupying one bounded per-Room admission lane position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionLaneClassV1 {
    /// A participant Action, which receives an admitted semantic time.
    ParticipantAction,
    /// A host stimulus such as a `TimerFired` candidate or administration.
    HostStimulus,
}

/// Why a lane reservation was not created.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AdmissionLaneErrorV1 {
    /// The lane has no position available for this class.
    #[error("Room Admission Lane is full")]
    Full,
    /// The `HostClock` failed after capacity was checked; no time was issued.
    #[error(transparent)]
    Clock(#[from] HostClockErrorV1),
    /// The configured lane has no usable participant or host capacity.
    #[error("Room Admission Lane capacity is invalid")]
    InvalidCapacity,
    /// The in-process lane coordinator cannot safely admit or order work.
    #[error("Room Admission Lane is unavailable")]
    Unavailable,
}

#[derive(Clone, Debug)]
struct LaneEntry {
    ticket: u64,
    class: AdmissionLaneClassV1,
    admitted_at: Option<HostClockSampleV1>,
}

/// One bounded FIFO Room Admission Lane.
pub struct RoomAdmissionLaneV1 {
    capacity: usize,
    host_reserve: usize,
    next_ticket: u64,
    entries: VecDeque<LaneEntry>,
}

impl fmt::Debug for RoomAdmissionLaneV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoomAdmissionLaneV1")
            .field("capacity", &self.capacity)
            .field("host_reserve", &self.host_reserve)
            .field("next_ticket", &self.next_ticket)
            .field("queued", &self.entries.len())
            .finish()
    }
}

impl RoomAdmissionLaneV1 {
    /// Creates a lane. `host_reserve` positions are unavailable to
    /// participants, ensuring due host work cannot be starved by Actions.
    ///
    /// # Errors
    ///
    /// Returns an error when the capacity is zero or the host reserve exceeds
    /// the total capacity.
    pub fn new(capacity: usize, host_reserve: usize) -> Result<Self, AdmissionLaneErrorV1> {
        if capacity == 0 || host_reserve > capacity {
            return Err(AdmissionLaneErrorV1::InvalidCapacity);
        }
        Ok(Self {
            capacity,
            host_reserve,
            next_ticket: 1,
            entries: VecDeque::new(),
        })
    }

    /// Returns the configured total capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the number of reserved host-stimulus positions.
    #[must_use]
    pub const fn host_reserve(&self) -> usize {
        self.host_reserve
    }

    /// Returns the number of currently reserved positions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the lane has no reservations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Reserves one Action and samples `HostClock` only after capacity admits it.
    /// A full lane therefore returns without a timestamp or deadline
    /// entitlement.
    ///
    /// # Errors
    ///
    /// Returns an error when the lane is full, the clock fails, or the ticket
    /// counter cannot advance.
    pub fn reserve_action(
        &mut self,
        clock: &dyn HostClockV1,
    ) -> Result<ActionLaneReservationV1, AdmissionLaneErrorV1> {
        let participant_capacity = self.capacity.saturating_sub(self.host_reserve);
        let participant_count = self
            .entries
            .iter()
            .filter(|entry| entry.class == AdmissionLaneClassV1::ParticipantAction)
            .count();
        if self.entries.len() >= self.capacity || participant_count >= participant_capacity {
            return Err(AdmissionLaneErrorV1::Full);
        }
        let ticket = self.push(AdmissionLaneClassV1::ParticipantAction)?;
        match clock.sample() {
            Ok(sample) => {
                let Some(entry) = self.entries.back_mut() else {
                    unreachable!("lane reservation was just appended")
                };
                entry.admitted_at = Some(sample.clone());
                Ok(ActionLaneReservationV1 {
                    ticket,
                    admitted_at: sample,
                })
            }
            Err(error) => {
                let Some(removed) = self.entries.pop_back() else {
                    unreachable!("lane reservation was just appended")
                };
                debug_assert_eq!(removed.ticket, ticket);
                Err(error.into())
            }
        }
    }

    /// Reserves one host-stimulus position without sampling a new semantic
    /// time. The Timer's immutable `scheduled_for` remains its semantic time.
    ///
    /// # Errors
    ///
    /// Returns an error when the lane is full or the ticket counter cannot
    /// advance.
    pub fn reserve_host_stimulus(&mut self) -> Result<LaneReservationV1, AdmissionLaneErrorV1> {
        if self.entries.len() >= self.capacity {
            return Err(AdmissionLaneErrorV1::Full);
        }
        let ticket = self.push(AdmissionLaneClassV1::HostStimulus)?;
        Ok(LaneReservationV1 {
            ticket,
            class: AdmissionLaneClassV1::HostStimulus,
        })
    }

    /// Removes and returns the oldest reservation. No later reservation can
    /// overtake it.
    pub fn dequeue(&mut self) -> Option<LaneReservationV1> {
        self.entries.pop_front().map(|entry| LaneReservationV1 {
            ticket: entry.ticket,
            class: entry.class,
        })
    }

    fn front_ticket(&self) -> Option<u64> {
        self.entries.front().map(|entry| entry.ticket)
    }

    /// Discards one still-queued provisional reservation, as on a crash before
    /// persistence handoff. No semantic time is retained by the lane.
    pub fn discard(&mut self, ticket: u64) -> bool {
        let Some(index) = self.entries.iter().position(|entry| entry.ticket == ticket) else {
            return false;
        };
        self.entries.remove(index).is_some()
    }

    fn push(&mut self, class: AdmissionLaneClassV1) -> Result<u64, AdmissionLaneErrorV1> {
        let ticket = self.next_ticket;
        self.next_ticket = self
            .next_ticket
            .checked_add(1)
            .ok_or(AdmissionLaneErrorV1::InvalidCapacity)?;
        self.entries.push_back(LaneEntry {
            ticket,
            class,
            admitted_at: None,
        });
        Ok(ticket)
    }
}

#[derive(Debug, Default)]
struct RoomAdmissionQueueMetricsV1 {
    process_current: AtomicUsize,
    process_high_water: AtomicUsize,
    unit_high_water: AtomicUsize,
    admitted_total: AtomicU64,
    completed_total: AtomicU64,
    full_total: AtomicU64,
}

impl RoomAdmissionQueueMetricsV1 {
    fn note_admitted(&self, unit_depth: usize) {
        let process_depth = self.process_current.fetch_add(1, Ordering::AcqRel) + 1;
        atomic_max(&self.process_high_water, process_depth);
        atomic_max(&self.unit_high_water, unit_depth);
        self.admitted_total.fetch_add(1, Ordering::Relaxed);
    }

    fn note_completed(&self) {
        let previous = self.process_current.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "an admitted Room turn must still be counted");
        self.completed_total.fetch_add(1, Ordering::Relaxed);
    }

    fn note_full(&self) {
        self.full_total.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self, capacity: usize) -> RoomAdmissionQueueSnapshotV1 {
        RoomAdmissionQueueSnapshotV1 {
            capacity,
            process_current: self.process_current.load(Ordering::Acquire),
            process_high_water: self.process_high_water.load(Ordering::Acquire),
            unit_high_water: self.unit_high_water.load(Ordering::Acquire),
            admitted_total: self.admitted_total.load(Ordering::Relaxed),
            completed_total: self.completed_total.load(Ordering::Relaxed),
            full_total: self.full_total.load(Ordering::Relaxed),
        }
    }
}

fn atomic_max(target: &AtomicUsize, candidate: usize) {
    let mut current = target.load(Ordering::Acquire);
    while candidate > current {
        match target.compare_exchange_weak(current, candidate, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

/// Low-cardinality process observation of all Room Admission Lanes.
///
/// The configured capacity and `unit_high_water` are per Room. Process
/// current/high-water values aggregate all Rooms and therefore are not
/// compared with the per-Room capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoomAdmissionQueueSnapshotV1 {
    pub capacity: usize,
    pub process_current: usize,
    pub process_high_water: usize,
    pub unit_high_water: usize,
    pub admitted_total: u64,
    pub completed_total: u64,
    pub full_total: u64,
}

impl Default for RoomAdmissionQueueSnapshotV1 {
    fn default() -> Self {
        Self {
            capacity: ROOM_ADMISSION_LANE_CAPACITY_V1,
            process_current: 0,
            process_high_water: 0,
            unit_high_water: 0,
            admitted_total: 0,
            completed_total: 0,
            full_total: 0,
        }
    }
}

#[derive(Debug)]
struct CoordinatedRoomLaneV1 {
    lane: Mutex<RoomAdmissionLaneV1>,
    changed: Condvar,
    metrics: Arc<RoomAdmissionQueueMetricsV1>,
}

impl CoordinatedRoomLaneV1 {
    fn new(
        capacity: usize,
        host_reserve: usize,
        metrics: Arc<RoomAdmissionQueueMetricsV1>,
    ) -> Result<Self, AdmissionLaneErrorV1> {
        Ok(Self {
            lane: Mutex::new(RoomAdmissionLaneV1::new(capacity, host_reserve)?),
            changed: Condvar::new(),
            metrics,
        })
    }

    fn reserve_action(
        self: &Arc<Self>,
        clock: &dyn HostClockV1,
    ) -> Result<ActionRoomAdmissionV1, AdmissionLaneErrorV1> {
        let (reservation, unit_depth) = {
            let mut lane = self
                .lane
                .lock()
                .map_err(|_| AdmissionLaneErrorV1::Unavailable)?;
            match lane.reserve_action(clock) {
                Ok(reservation) => (reservation, lane.len()),
                Err(error) => {
                    if error == AdmissionLaneErrorV1::Full {
                        self.metrics.note_full();
                    }
                    return Err(error);
                }
            }
        };
        self.metrics.note_admitted(unit_depth);
        let admitted_at = reservation.semantic_time();
        if let Err(error) = self.wait_for_turn(reservation.ticket()) {
            self.metrics.note_completed();
            return Err(error);
        }
        Ok(ActionRoomAdmissionV1 {
            turn: RoomAdmissionTurnV1 {
                lane: Arc::clone(self),
                ticket: reservation.ticket(),
                class: AdmissionLaneClassV1::ParticipantAction,
            },
            admitted_at,
        })
    }

    fn reserve_host_stimulus(
        self: &Arc<Self>,
    ) -> Result<RoomAdmissionTurnV1, AdmissionLaneErrorV1> {
        let (reservation, unit_depth) = {
            let mut lane = self
                .lane
                .lock()
                .map_err(|_| AdmissionLaneErrorV1::Unavailable)?;
            match lane.reserve_host_stimulus() {
                Ok(reservation) => (reservation, lane.len()),
                Err(error) => {
                    if error == AdmissionLaneErrorV1::Full {
                        self.metrics.note_full();
                    }
                    return Err(error);
                }
            }
        };
        self.metrics.note_admitted(unit_depth);
        if let Err(error) = self.wait_for_turn(reservation.ticket()) {
            self.metrics.note_completed();
            return Err(error);
        }
        Ok(RoomAdmissionTurnV1 {
            lane: Arc::clone(self),
            ticket: reservation.ticket(),
            class: reservation.class(),
        })
    }

    fn wait_for_turn(&self, ticket: u64) -> Result<(), AdmissionLaneErrorV1> {
        let mut lane = match self.lane.lock() {
            Ok(lane) => lane,
            Err(poisoned) => {
                let mut lane = poisoned.into_inner();
                let _ = lane.discard(ticket);
                self.changed.notify_all();
                return Err(AdmissionLaneErrorV1::Unavailable);
            }
        };
        while lane.front_ticket() != Some(ticket) {
            lane = match self.changed.wait(lane) {
                Ok(lane) => lane,
                Err(poisoned) => {
                    let mut lane = poisoned.into_inner();
                    let _ = lane.discard(ticket);
                    self.changed.notify_all();
                    return Err(AdmissionLaneErrorV1::Unavailable);
                }
            };
        }
        Ok(())
    }

    fn depth(&self) -> Result<usize, AdmissionLaneErrorV1> {
        self.lane
            .lock()
            .map(|lane| lane.len())
            .map_err(|_| AdmissionLaneErrorV1::Unavailable)
    }

    fn release(&self, ticket: u64) {
        let mut lane = match self.lane.lock() {
            Ok(lane) => lane,
            Err(poisoned) => poisoned.into_inner(),
        };
        let removed = if lane.front_ticket() == Some(ticket) {
            lane.dequeue().is_some()
        } else {
            lane.discard(ticket)
        };
        debug_assert!(removed, "admission turn must retain its provisional ticket");
        if removed {
            self.metrics.note_completed();
        }
        self.changed.notify_all();
    }
}

/// Process-local coordinators for the bounded FIFO admission lane of every
/// loaded Room.
///
/// Reserving returns only when that ticket is at the front of its Room lane.
/// The returned turn retains the position until it is dropped, so a later
/// Action or host stimulus cannot begin its commit before an earlier one.
pub struct RoomAdmissionLanesV1 {
    capacity: usize,
    host_reserve: usize,
    rooms: Mutex<BTreeMap<RoomId, Weak<CoordinatedRoomLaneV1>>>,
    metrics: Arc<RoomAdmissionQueueMetricsV1>,
}

impl fmt::Debug for RoomAdmissionLanesV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoomAdmissionLanesV1")
            .field("capacity", &self.capacity)
            .field("host_reserve", &self.host_reserve)
            .field("metrics", &self.metrics.snapshot(self.capacity))
            .field("rooms", &"[OPAQUE]")
            .finish()
    }
}

impl Default for RoomAdmissionLanesV1 {
    fn default() -> Self {
        Self::new(
            ROOM_ADMISSION_LANE_CAPACITY_V1,
            ROOM_ADMISSION_HOST_RESERVE_V1,
        )
        .unwrap_or_else(|_| unreachable!("frozen Room admission configuration is valid"))
    }
}

impl RoomAdmissionLanesV1 {
    /// Creates a per-Room lane set with one shared capacity policy.
    ///
    /// # Errors
    ///
    /// Returns an error when the lane capacity or host reserve is invalid.
    pub fn new(capacity: usize, host_reserve: usize) -> Result<Self, AdmissionLaneErrorV1> {
        let _ = RoomAdmissionLaneV1::new(capacity, host_reserve)?;
        Ok(Self {
            capacity,
            host_reserve,
            rooms: Mutex::new(BTreeMap::new()),
            metrics: Arc::new(RoomAdmissionQueueMetricsV1::default()),
        })
    }

    /// Returns the configured capacity of each Room lane.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the positions reserved for host stimuli in each Room lane.
    #[must_use]
    pub const fn host_reserve(&self) -> usize {
        self.host_reserve
    }

    /// Atomically reserves Action capacity and samples its semantic time, then
    /// waits for the reservation's FIFO turn.
    ///
    /// # Errors
    ///
    /// Returns `Full` before sampling the clock when participant capacity is
    /// exhausted, or a closed clock/coordinator error.
    pub fn reserve_action(
        &self,
        room_id: &RoomId,
        clock: &dyn HostClockV1,
    ) -> Result<ActionRoomAdmissionV1, AdmissionLaneErrorV1> {
        self.room_lane(room_id)?.reserve_action(clock)
    }

    /// Reserves host-stimulus capacity, then waits for the reservation's FIFO
    /// turn.
    ///
    /// # Errors
    ///
    /// Returns `Full` when all Room positions are occupied, or `Unavailable`
    /// when the coordinator cannot safely preserve ordering.
    pub fn reserve_host_stimulus(
        &self,
        room_id: &RoomId,
    ) -> Result<RoomAdmissionTurnV1, AdmissionLaneErrorV1> {
        self.room_lane(room_id)?.reserve_host_stimulus()
    }

    /// Returns the current provisional position count for one Room.
    ///
    /// # Errors
    ///
    /// Returns `Unavailable` when the coordinator cannot safely inspect the
    /// Room lane.
    pub fn room_depth(&self, room_id: &RoomId) -> Result<usize, AdmissionLaneErrorV1> {
        let lane = self
            .rooms
            .lock()
            .map_err(|_| AdmissionLaneErrorV1::Unavailable)?
            .get(room_id)
            .and_then(Weak::upgrade);
        lane.map_or(Ok(0), |lane| lane.depth())
    }

    /// Returns fixed-cardinality process counters for public queue metrics.
    #[must_use]
    pub fn queue_snapshot(&self) -> RoomAdmissionQueueSnapshotV1 {
        self.metrics.snapshot(self.capacity)
    }

    fn room_lane(
        &self,
        room_id: &RoomId,
    ) -> Result<Arc<CoordinatedRoomLaneV1>, AdmissionLaneErrorV1> {
        let mut rooms = self
            .rooms
            .lock()
            .map_err(|_| AdmissionLaneErrorV1::Unavailable)?;
        if let Some(lane) = rooms.get(room_id).and_then(Weak::upgrade) {
            return Ok(lane);
        }
        rooms.retain(|_, lane| lane.strong_count() > 0);
        let lane = Arc::new(CoordinatedRoomLaneV1::new(
            self.capacity,
            self.host_reserve,
            Arc::clone(&self.metrics),
        )?);
        rooms.insert(room_id.clone(), Arc::downgrade(&lane));
        Ok(lane)
    }
}

/// One Action's active FIFO turn and the semantic sample issued with its
/// successful capacity reservation.
#[must_use = "the Action admission must be retained through its Room commit"]
pub struct ActionRoomAdmissionV1 {
    turn: RoomAdmissionTurnV1,
    admitted_at: ActionAdmittedAt,
}

impl fmt::Debug for ActionRoomAdmissionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActionRoomAdmissionV1")
            .field("ticket", &self.turn.ticket)
            .field("admitted_at", &self.admitted_at)
            .finish()
    }
}

impl ActionRoomAdmissionV1 {
    /// Returns the exact Action semantic time sampled with lane reservation.
    #[must_use]
    pub const fn admitted_at(&self) -> &ActionAdmittedAt {
        &self.admitted_at
    }

    /// Returns the FIFO ticket retained by this turn.
    #[must_use]
    pub const fn ticket(&self) -> u64 {
        self.turn.ticket
    }
}

/// An active FIFO Room-lane turn. Dropping it releases the provisional
/// position and wakes the next reservation.
#[must_use = "the admission turn must be retained through its Room commit"]
pub struct RoomAdmissionTurnV1 {
    lane: Arc<CoordinatedRoomLaneV1>,
    ticket: u64,
    class: AdmissionLaneClassV1,
}

impl fmt::Debug for RoomAdmissionTurnV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoomAdmissionTurnV1")
            .field("lane", &"[OPAQUE]")
            .field("ticket", &self.ticket)
            .field("class", &self.class)
            .finish()
    }
}

impl RoomAdmissionTurnV1 {
    /// Returns this provisional FIFO ticket.
    #[must_use]
    pub const fn ticket(&self) -> u64 {
        self.ticket
    }

    /// Returns the source class occupying this position.
    #[must_use]
    pub const fn class(&self) -> AdmissionLaneClassV1 {
        self.class
    }
}

impl Drop for RoomAdmissionTurnV1 {
    fn drop(&mut self) {
        self.lane.release(self.ticket);
    }
}

/// A dequeued or discarded lane position.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaneReservationV1 {
    ticket: u64,
    class: AdmissionLaneClassV1,
}

impl LaneReservationV1 {
    /// Returns the provisional lane ticket.
    #[must_use]
    pub const fn ticket(self) -> u64 {
        self.ticket
    }

    /// Returns the admitted source class.
    #[must_use]
    pub const fn class(self) -> AdmissionLaneClassV1 {
        self.class
    }
}

/// A successful Action reservation with its one `HostClock` admission sample.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionLaneReservationV1 {
    ticket: u64,
    admitted_at: HostClockSampleV1,
}

impl ActionLaneReservationV1 {
    /// Returns the provisional lane ticket.
    #[must_use]
    pub const fn ticket(&self) -> u64 {
        self.ticket
    }

    /// Returns the exact host-recorded Action admission time.
    #[must_use]
    pub const fn admitted_at(&self) -> &HostClockSampleV1 {
        &self.admitted_at
    }

    /// Converts the reservation's sample to the canonical Action time field.
    #[must_use]
    pub fn semantic_time(&self) -> ActionAdmittedAt {
        self.admitted_at.as_action_admitted_at()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    struct FakeClock {
        samples: Vec<Result<HostClockSampleV1, HostClockErrorV1>>,
        calls: AtomicUsize,
    }

    impl HostClockV1 for FakeClock {
        fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
            let index = self.calls.fetch_add(1, Ordering::Relaxed);
            self.samples[index].clone()
        }
    }

    fn sample(value: &str) -> HostClockSampleV1 {
        HostClockSampleV1::new(value).unwrap_or_else(|error| panic!("sample: {error}"))
    }

    fn room() -> RoomId {
        "01ARZ3NDEKTSV4RRFFQ69G5FAV"
            .parse()
            .unwrap_or_else(|error| panic!("Room ID: {error}"))
    }

    fn wait_for_depth(lanes: &RoomAdmissionLanesV1, room_id: &RoomId, expected: usize) {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            if lanes.room_depth(room_id) == Ok(expected) {
                return;
            }
            thread::yield_now();
        }
        panic!("Room lane did not reach depth {expected}");
    }

    #[test]
    fn full_action_lane_does_not_sample_or_issue_deadline_time() {
        let clock = FakeClock {
            samples: vec![Ok(sample("2026-08-20T12:00:00Z"))],
            calls: AtomicUsize::new(0),
        };
        let mut lane =
            RoomAdmissionLaneV1::new(1, 0).unwrap_or_else(|error| panic!("lane: {error}"));
        assert!(lane.reserve_action(&clock).is_ok());
        assert_eq!(lane.reserve_action(&clock), Err(AdmissionLaneErrorV1::Full));
        assert_eq!(clock.calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn host_reserve_preserves_fifo_order_and_prevents_participant_starvation() {
        let clock = FakeClock {
            samples: vec![Ok(sample("2026-08-20T12:00:00Z"))],
            calls: AtomicUsize::new(0),
        };
        let mut lane =
            RoomAdmissionLaneV1::new(2, 1).unwrap_or_else(|error| panic!("lane: {error}"));
        assert!(lane.reserve_action(&clock).is_ok());
        assert_eq!(lane.reserve_action(&clock), Err(AdmissionLaneErrorV1::Full));
        assert_eq!(
            lane.reserve_host_stimulus().map(|r| r.class()),
            Ok(AdmissionLaneClassV1::HostStimulus)
        );
        assert_eq!(
            lane.dequeue().map(|r| r.class()),
            Some(AdmissionLaneClassV1::ParticipantAction)
        );
        assert_eq!(
            lane.dequeue().map(|r| r.class()),
            Some(AdmissionLaneClassV1::HostStimulus)
        );
    }

    #[test]
    fn occupied_host_positions_do_not_reduce_the_participant_quota_twice() {
        let clock = FakeClock {
            samples: vec![
                Ok(sample("2026-08-20T12:00:00Z")),
                Ok(sample("2026-08-20T12:00:01Z")),
            ],
            calls: AtomicUsize::new(0),
        };
        let mut lane =
            RoomAdmissionLaneV1::new(3, 1).unwrap_or_else(|error| panic!("lane: {error}"));
        assert!(lane.reserve_host_stimulus().is_ok());
        assert!(lane.reserve_action(&clock).is_ok());
        assert!(lane.reserve_action(&clock).is_ok());
        assert_eq!(lane.reserve_action(&clock), Err(AdmissionLaneErrorV1::Full));
        assert_eq!(lane.len(), 3);
        assert_eq!(clock.calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn coordinated_lanes_do_not_retain_inactive_room_state() {
        let clock = FakeClock {
            samples: vec![
                Ok(sample("2026-08-20T12:00:00Z")),
                Ok(sample("2026-08-20T12:00:01Z")),
            ],
            calls: AtomicUsize::new(0),
        };
        let lanes = RoomAdmissionLanesV1::new(2, 1)
            .unwrap_or_else(|error| panic!("coordinated lane: {error}"));
        let first_room = room();
        let first = lanes
            .reserve_action(&first_room, &clock)
            .unwrap_or_else(|error| panic!("first room: {error}"));
        drop(first);

        let second_room = "01ARZ3NDEKTSV4RRFFQ69G5FAW"
            .parse()
            .unwrap_or_else(|error| panic!("second Room ID: {error}"));
        let second = lanes
            .reserve_action(&second_room, &clock)
            .unwrap_or_else(|error| panic!("second room: {error}"));
        assert_eq!(
            lanes
                .rooms
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .len(),
            1
        );
        drop(second);
    }

    #[test]
    fn failed_clock_sample_releases_the_provisional_lane_position() {
        let clock = FakeClock {
            samples: vec![Err(HostClockErrorV1::Unavailable)],
            calls: AtomicUsize::new(0),
        };
        let mut lane =
            RoomAdmissionLaneV1::new(1, 0).unwrap_or_else(|error| panic!("lane: {error}"));
        assert_eq!(
            lane.reserve_action(&clock),
            Err(AdmissionLaneErrorV1::Clock(HostClockErrorV1::Unavailable))
        );
        assert!(lane.is_empty());
    }

    #[test]
    fn coordinated_lane_rejects_full_action_before_sampling() {
        let clock = FakeClock {
            samples: vec![Ok(sample("2026-08-20T12:00:00Z"))],
            calls: AtomicUsize::new(0),
        };
        let lanes = RoomAdmissionLanesV1::new(2, 1)
            .unwrap_or_else(|error| panic!("coordinated lane: {error}"));
        let room_id = room();
        let admission = lanes
            .reserve_action(&room_id, &clock)
            .unwrap_or_else(|error| panic!("first Action: {error}"));

        assert!(matches!(
            lanes.reserve_action(&room_id, &clock),
            Err(AdmissionLaneErrorV1::Full)
        ));
        assert_eq!(clock.calls.load(Ordering::Relaxed), 1);
        assert_eq!(lanes.room_depth(&room_id), Ok(1));
        assert_eq!(
            lanes.queue_snapshot(),
            RoomAdmissionQueueSnapshotV1 {
                capacity: 2,
                process_current: 1,
                process_high_water: 1,
                unit_high_water: 1,
                admitted_total: 1,
                completed_total: 0,
                full_total: 1,
            }
        );
        drop(admission);
        assert_eq!(lanes.room_depth(&room_id), Ok(0));
        assert_eq!(lanes.queue_snapshot().process_current, 0);
        assert_eq!(lanes.queue_snapshot().completed_total, 1);
    }

    #[test]
    fn coordinated_lane_holds_fifo_turn_through_action_and_host_work() {
        let lanes = Arc::new(
            RoomAdmissionLanesV1::new(4, 1)
                .unwrap_or_else(|error| panic!("coordinated lane: {error}")),
        );
        let clock = Arc::new(FakeClock {
            samples: vec![
                Ok(sample("2026-08-20T12:00:00Z")),
                Ok(sample("2026-08-20T12:00:01Z")),
            ],
            calls: AtomicUsize::new(0),
        });
        let room_id = room();
        let first = lanes
            .reserve_action(&room_id, clock.as_ref())
            .unwrap_or_else(|error| panic!("first Action: {error}"));

        let (host_acquired_tx, host_acquired_rx) = mpsc::channel();
        let (release_host_tx, release_host_rx) = mpsc::channel();
        let host_lanes = Arc::clone(&lanes);
        let host_room = room_id.clone();
        let host = thread::spawn(move || {
            let admission = host_lanes
                .reserve_host_stimulus(&host_room)
                .unwrap_or_else(|error| panic!("host admission: {error}"));
            host_acquired_tx
                .send(())
                .unwrap_or_else(|error| panic!("announce host turn: {error}"));
            release_host_rx
                .recv()
                .unwrap_or_else(|error| panic!("release host turn: {error}"));
            drop(admission);
        });
        wait_for_depth(&lanes, &room_id, 2);

        let (action_acquired_tx, action_acquired_rx) = mpsc::channel();
        let action_lanes = Arc::clone(&lanes);
        let action_clock = Arc::clone(&clock);
        let action_room = room_id.clone();
        let action = thread::spawn(move || {
            let admission = action_lanes
                .reserve_action(&action_room, action_clock.as_ref())
                .unwrap_or_else(|error| panic!("second Action admission: {error}"));
            action_acquired_tx
                .send(())
                .unwrap_or_else(|error| panic!("announce Action turn: {error}"));
            drop(admission);
        });
        wait_for_depth(&lanes, &room_id, 3);

        assert!(matches!(
            host_acquired_rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert!(matches!(
            action_acquired_rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));

        drop(first);
        host_acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("host should receive second turn: {error}"));
        assert!(matches!(
            action_acquired_rx.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        release_host_tx
            .send(())
            .unwrap_or_else(|error| panic!("release host: {error}"));
        action_acquired_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| panic!("Action should receive third turn: {error}"));
        host.join()
            .unwrap_or_else(|_| panic!("host admission thread"));
        action
            .join()
            .unwrap_or_else(|_| panic!("Action admission thread"));
        assert_eq!(lanes.room_depth(&room_id), Ok(0));
    }

    #[test]
    fn monotonic_clock_rejects_rollback_without_rewriting_the_last_sample() {
        struct Source(Mutex<VecDeque<HostClockSampleV1>>);
        impl HostClockV1 for Source {
            fn sample(&self) -> Result<HostClockSampleV1, HostClockErrorV1> {
                self.0
                    .lock()
                    .map_err(|_| HostClockErrorV1::Unavailable)?
                    .pop_front()
                    .ok_or(HostClockErrorV1::Unavailable)
            }
        }
        let clock = MonotonicHostClockV1::new(Source(Mutex::new(VecDeque::from([
            sample("2026-08-20T12:00:01Z"),
            sample("2026-08-20T11:59:59Z"),
        ]))));
        assert_eq!(
            clock.sample().map(|s| s.as_str().to_owned()),
            Ok("2026-08-20T12:00:01Z".to_owned())
        );
        assert_eq!(clock.sample(), Err(HostClockErrorV1::Rollback));
    }
}

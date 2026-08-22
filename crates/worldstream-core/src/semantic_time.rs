//! Host-owned semantic time and bounded Room admission primitives.

#![cfg_attr(test, allow(clippy::panic, clippy::redundant_closure_for_method_calls))]

use std::{collections::VecDeque, fmt, sync::Mutex};

use crate::primitives::compare_timestamp_text;
use crate::{ActionAdmittedAt, TimerScheduledFor};

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
        if self.entries.len() >= self.capacity.saturating_sub(self.host_reserve) {
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

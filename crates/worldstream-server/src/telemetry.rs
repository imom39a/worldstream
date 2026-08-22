//! Vendor-neutral, post-commit telemetry primitives.
//!
//! This module deliberately accepts only bounded, typed facts. It has no
//! dependency on a collector SDK and no connection to the Room commit path.
//! Callers should submit events after the authoritative result is known.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use worldstream_postgres::{
    PostgresIntegrityStatusV1, PostgresMigrationPhaseV1, PostgresRecoveryPhaseV1,
    PostgresStorageDiagnosticKindV1, PostgresTelemetryEventV1, PostgresTelemetrySink,
};
use worldstream_sqlite::{
    SqliteIntegrityStatusV1, SqliteMigrationPhaseV1, SqliteRecoveryPhaseV1,
    SqliteStorageDiagnosticKindV1, SqliteTelemetryEventV1, SqliteTelemetrySink,
};

/// Stable schema identity for all events emitted by this module.
pub const TELEMETRY_SCHEMA_V1: &str = "worldstream/telemetry/v1";
/// Maximum encoded size of one structured event.
pub const MAX_EVENT_BYTES: usize = 4096;
/// Maximum number of queued events for one exporter worker.
pub const MAX_QUEUE_CAPACITY: usize = 65_536;
const MAX_ATTRIBUTES: usize = 16;
const MAX_INPUT_ATTRIBUTES: usize = MAX_ATTRIBUTES * 16;
const MAX_ATTRIBUTE_VALUE: u64 = 1_000_000_000;
const MAX_ENDPOINT_BYTES: usize = 256;
const MAX_BATCH_SIZE: usize = 128;
const MAX_SHUTDOWN_FLUSH: Duration = Duration::from_secs(3);
const OVERFLOW_WARNING_INTERVAL: Duration = Duration::from_secs(1);
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const HTTP_IO_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HTTP_REQUEST_BODY_BYTES: usize = MAX_EVENT_BYTES * MAX_BATCH_SIZE + 16 * 1024;
const MAX_HTTP_RESPONSE_BYTES: usize = 8192;
const DNS_RESOLVER_WORKERS: usize = 2;
const DNS_RESOLVER_QUEUE_PER_WORKER: usize = 1;
pub(crate) const DEFAULT_QUEUE_CAPACITY: usize = 256;

#[derive(Debug, Default)]
struct DnsQueueCounters {
    current: AtomicUsize,
    high_water: AtomicUsize,
    submitted: AtomicU64,
    completed: AtomicU64,
    rejected: AtomicU64,
}

impl DnsQueueCounters {
    fn note_submitted(&self) {
        let depth = self.current.fetch_add(1, Ordering::AcqRel) + 1;
        atomic_max_usize(&self.high_water, depth);
        self.submitted.fetch_add(1, Ordering::Relaxed);
    }

    fn note_dequeued(&self) {
        let previous = self.current.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "a submitted DNS request must still be queued");
    }

    fn note_completed(&self) {
        self.completed.fetch_add(1, Ordering::Relaxed);
    }

    fn note_rejected(&self) {
        self.rejected.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self, capacity: usize) -> crate::InternalQueueSnapshot {
        let high_water = self.high_water.load(Ordering::Acquire);
        crate::InternalQueueSnapshot {
            name: "telemetry_dns_resolver_queue",
            capacity_scope: "global",
            capacity,
            process_current: self.current.load(Ordering::Acquire),
            process_high_water: high_water,
            unit_high_water: high_water,
            activity_total: self.submitted.load(Ordering::Relaxed),
            completion_total: self.completed.load(Ordering::Relaxed),
            backpressure_total: self.rejected.load(Ordering::Relaxed),
        }
    }
}

fn atomic_max_usize(target: &AtomicUsize, candidate: usize) {
    let mut current = target.load(Ordering::Acquire);
    while candidate > current {
        match target.compare_exchange_weak(current, candidate, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn atomic_max_u64(target: &AtomicU64, candidate: u64) {
    let mut current = target.load(Ordering::Acquire);
    while candidate > current {
        match target.compare_exchange_weak(current, candidate, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

type DnsLookup = Arc<dyn Fn(String) -> Vec<SocketAddr> + Send + Sync>;

struct DnsRequest {
    connect_target: String,
    response: SyncSender<Vec<SocketAddr>>,
    submitted: Arc<AtomicBool>,
}

struct DnsResolverPool {
    workers: Vec<SyncSender<DnsRequest>>,
    next_worker: AtomicUsize,
    queue_capacity: usize,
    metrics: Arc<DnsQueueCounters>,
}

impl DnsResolverPool {
    fn new() -> Result<Self, ExportError> {
        let lookup: DnsLookup = Arc::new(|connect_target| {
            connect_target
                .to_socket_addrs()
                .map_or_else(|_| Vec::new(), Iterator::collect)
        });
        Self::with_lookup(DNS_RESOLVER_WORKERS, DNS_RESOLVER_QUEUE_PER_WORKER, &lookup)
    }

    fn with_lookup(
        worker_count: usize,
        queue_capacity: usize,
        lookup: &DnsLookup,
    ) -> Result<Self, ExportError> {
        if worker_count == 0 || queue_capacity == 0 {
            return Err(ExportError::Unavailable);
        }
        let metrics = Arc::new(DnsQueueCounters::default());
        let mut workers = Vec::with_capacity(worker_count);
        for index in 0..worker_count {
            let (sender, receiver) = mpsc::sync_channel::<DnsRequest>(queue_capacity);
            let worker_lookup = Arc::clone(lookup);
            let worker_metrics = Arc::clone(&metrics);
            thread::Builder::new()
                .name(format!("worldstream-telemetry-dns-{index}"))
                .spawn(move || {
                    while let Ok(request) = receiver.recv() {
                        while !request.submitted.load(Ordering::Acquire) {
                            thread::yield_now();
                        }
                        worker_metrics.note_dequeued();
                        let addresses = worker_lookup(request.connect_target);
                        let _ = request.response.send(addresses);
                        worker_metrics.note_completed();
                    }
                })
                .map_err(|_| ExportError::Unavailable)?;
            workers.push(sender);
        }
        Ok(Self {
            workers,
            next_worker: AtomicUsize::new(0),
            queue_capacity: worker_count.saturating_mul(queue_capacity),
            metrics,
        })
    }

    fn resolve(
        &self,
        connect_target: String,
        timeout: Duration,
    ) -> Result<Vec<SocketAddr>, ExportError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let submitted_signal = Arc::new(AtomicBool::new(false));
        let mut request = DnsRequest {
            connect_target,
            response: sender,
            submitted: Arc::clone(&submitted_signal),
        };
        let start = self.next_worker.fetch_add(1, Ordering::Relaxed);
        let mut was_submitted = false;
        for offset in 0..self.workers.len() {
            let index = start.wrapping_add(offset) % self.workers.len();
            match self.workers[index].try_send(request) {
                Ok(()) => {
                    self.metrics.note_submitted();
                    submitted_signal.store(true, Ordering::Release);
                    was_submitted = true;
                    break;
                }
                Err(TrySendError::Full(returned) | TrySendError::Disconnected(returned)) => {
                    request = returned;
                }
            }
        }
        if !was_submitted {
            self.metrics.note_rejected();
            return Err(ExportError::Unavailable);
        }
        receiver
            .recv_timeout(timeout)
            .ok()
            .filter(|addresses| !addresses.is_empty())
            .ok_or(ExportError::Unavailable)
    }

    fn queue_snapshot(&self) -> crate::InternalQueueSnapshot {
        self.metrics.snapshot(self.queue_capacity)
    }
}

static DNS_RESOLVER_POOL: OnceLock<Option<DnsResolverPool>> = OnceLock::new();

pub(crate) fn dns_resolver_queue_snapshot() -> crate::InternalQueueSnapshot {
    DNS_RESOLVER_POOL
        .get()
        .and_then(Option::as_ref)
        .map_or_else(
            || {
                DnsQueueCounters::default()
                    .snapshot(DNS_RESOLVER_WORKERS.saturating_mul(DNS_RESOLVER_QUEUE_PER_WORKER))
            },
            DnsResolverPool::queue_snapshot,
        )
}

/// The bounded, fixed-cardinality event families supported by the seam.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKindV1 {
    Admission,
    CommitOutcome,
    Timer,
    FrameDelivery,
    Activation,
    Recovery,
    Migration,
    StorageDiagnostic,
}

impl EventKindV1 {
    const ALL: [Self; 8] = [
        Self::Admission,
        Self::CommitOutcome,
        Self::Timer,
        Self::FrameDelivery,
        Self::Activation,
        Self::Recovery,
        Self::Migration,
        Self::StorageDiagnostic,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Admission => 0,
            Self::CommitOutcome => 1,
            Self::Timer => 2,
            Self::FrameDelivery => 3,
            Self::Activation => 4,
            Self::Recovery => 5,
            Self::Migration => 6,
            Self::StorageDiagnostic => 7,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Admission => "admission",
            Self::CommitOutcome => "commit_outcome",
            Self::Timer => "timer",
            Self::FrameDelivery => "frame_delivery",
            Self::Activation => "activation",
            Self::Recovery => "recovery",
            Self::Migration => "migration",
            Self::StorageDiagnostic => "storage_diagnostic",
        }
    }
}

/// Closed reason vocabulary shared by events and exporter diagnostics.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCodeV1 {
    Accepted,
    Rejected,
    Conflict,
    Busy,
    Unauthorized,
    Invalid,
    StorageUnavailable,
    CommitIndeterminate,
    Timeout,
    RetryExhausted,
    QueueFull,
    ExporterUnavailable,
    ExporterMalformedEndpoint,
    CollectorSlow,
    MigrationMismatch,
    SchemaMismatch,
    IntegrityFailure,
    RecoveryRequired,
    RecoveryComplete,
    RoomUnhealthy,
    ProcessUnhealthy,
}

/// Optional adapter attribution for diagnostics. This is deliberately a
/// closed telemetry-only vocabulary: it never changes Room truth, hashes, or
/// protocol projections. Generic callers may leave it absent when the
/// selected backend is not authoritative at the gateway boundary.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterIdentityV1 {
    Sqlite,
    Postgres,
}

/// A parsed W3C `traceparent`. Business/entity identifiers are not part of
/// correlation; this value is only for request trace propagation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TraceParentV1 {
    version: u8,
    trace_id: [u8; 16],
    parent_id: [u8; 8],
    trace_flags: u8,
}

impl TraceParentV1 {
    /// Parses a W3C traceparent without accepting whitespace or zero IDs.
    ///
    /// # Errors
    ///
    /// Returns an error when the header shape, casing, version, or IDs are invalid.
    pub fn parse(value: &str) -> Result<Self, TraceParentError> {
        let parts: Vec<&str> = value.split('-').collect();
        if parts.len() != 4 {
            return Err(TraceParentError::Malformed);
        }
        if parts[0].len() != 2
            || parts[1].len() != 32
            || parts[2].len() != 16
            || parts[3].len() != 2
        {
            return Err(TraceParentError::Malformed);
        }
        let version = decode_hex_byte(parts[0])?;
        if version == u8::MAX {
            return Err(TraceParentError::Malformed);
        }
        let trace_id = decode_hex::<16>(parts[1])?;
        let parent_id = decode_hex::<8>(parts[2])?;
        let flags = decode_hex_byte(&parts[3][..2])?;
        if trace_id.iter().all(|byte| *byte == 0) || parent_id.iter().all(|byte| *byte == 0) {
            return Err(TraceParentError::ZeroIdentifier);
        }
        Ok(Self {
            version,
            trace_id,
            parent_id,
            trace_flags: flags,
        })
    }

    /// Builds a child context with a caller-provided deterministic span ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied span ID is all zeroes.
    pub fn child(self, parent_id: [u8; 8]) -> Result<Self, TraceParentError> {
        if parent_id.iter().all(|byte| *byte == 0) {
            return Err(TraceParentError::ZeroIdentifier);
        }
        Ok(Self { parent_id, ..self })
    }

    /// Returns the canonical W3C header value.
    #[must_use]
    pub fn to_header(self) -> String {
        format!(
            "{}-{}-{}-{}",
            hex(self.version.to_be_bytes()),
            hex(self.trace_id),
            hex(self.parent_id),
            hex([self.trace_flags]),
        )
    }
}

impl Serialize for TraceParentV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_header())
    }
}

impl<'de> Deserialize<'de> for TraceParentV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?.as_str()).map_err(D::Error::custom)
    }
}

/// Errors from strict W3C traceparent parsing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceParentError {
    Malformed,
    ZeroIdentifier,
}

impl std::fmt::Display for TraceParentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Malformed => "malformed traceparent",
            Self::ZeroIdentifier => "traceparent contains a zero identifier",
        })
    }
}

impl std::error::Error for TraceParentError {}

/// Correlation carried by a structured event.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CorrelationV1 {
    pub traceparent: Option<TraceParentV1>,
}

impl CorrelationV1 {
    #[must_use]
    pub const fn none() -> Self {
        Self { traceparent: None }
    }

    #[must_use]
    pub const fn from_traceparent(traceparent: TraceParentV1) -> Self {
        Self {
            traceparent: Some(traceparent),
        }
    }
}

/// Admission facts. No request body, clue, commitment, capability, or ID is
/// representable here.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionOutcomeV1 {
    Accepted,
    Rejected,
    Conflict,
    Unauthorized,
    Busy,
}

/// Commit result facts, deliberately separate from domain `Outcome` values.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitOutcomeV1 {
    Committed,
    Rejected,
    NoChange,
    Conflict,
    Fenced,
    Indeterminate,
    RetryableKnownAbsent,
    Fault,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TimerPhaseV1 {
    Scanned,
    Enqueued,
    Retried,
    Fired,
    Obsolete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameDeliveryOutcomeV1 {
    Queued,
    Delivered,
    Acknowledged,
    ResetRequired,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationPhaseV1 {
    IntentCreated,
    Claimed,
    Leased,
    Attempted,
    Completed,
    Fenced,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPhaseV1 {
    Started,
    IntegrityChecked,
    RoomIsolated,
    Replayed,
    Installed,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MigrationPhaseV1 {
    Started,
    Applied,
    AlreadyCurrent,
    Mismatch,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageDiagnosticKindV1 {
    Capacity,
    Connection,
    Integrity,
    Lock,
    Query,
    Checkpoint,
}

/// Typed event details for all supported event families.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub enum EventDetailsV1 {
    Admission { outcome: AdmissionOutcomeV1 },
    CommitOutcome { outcome: CommitOutcomeV1 },
    Timer { phase: TimerPhaseV1 },
    FrameDelivery { outcome: FrameDeliveryOutcomeV1 },
    Activation { phase: ActivationPhaseV1 },
    Recovery { phase: RecoveryPhaseV1 },
    Migration { phase: MigrationPhaseV1 },
    StorageDiagnostic { kind: StorageDiagnosticKindV1 },
}

impl EventDetailsV1 {
    const fn event_kind(self) -> EventKindV1 {
        match self {
            Self::Admission { .. } => EventKindV1::Admission,
            Self::CommitOutcome { .. } => EventKindV1::CommitOutcome,
            Self::Timer { .. } => EventKindV1::Timer,
            Self::FrameDelivery { .. } => EventKindV1::FrameDelivery,
            Self::Activation { .. } => EventKindV1::Activation,
            Self::Recovery { .. } => EventKindV1::Recovery,
            Self::Migration { .. } => EventKindV1::Migration,
            Self::StorageDiagnostic { .. } => EventKindV1::StorageDiagnostic,
        }
    }
}

/// Untrusted, pre-redaction input. Only allowlisted numeric attributes survive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryDraftV1 {
    pub event: EventKindV1,
    pub reason: ReasonCodeV1,
    pub correlation: CorrelationV1,
    pub adapter: Option<AdapterIdentityV1>,
    pub details: EventDetailsV1,
    pub observed_at_ms: Option<u64>,
    pub attributes: Vec<(String, String)>,
}

/// Adapts storage-owned `SQLite` facts to the bounded server telemetry schema.
/// The adapter is deliberately one-way: storage can enqueue a fact, but it
/// cannot observe exporter health or make an operation depend on delivery.
#[derive(Clone)]
pub struct SqliteTelemetryBridge {
    handle: TelemetryHandle,
}

impl SqliteTelemetryBridge {
    #[must_use]
    pub fn new(handle: TelemetryHandle) -> Self {
        Self { handle }
    }
}

impl SqliteTelemetrySink for SqliteTelemetryBridge {
    fn emit(&self, event: SqliteTelemetryEventV1) {
        let (details, reason, attributes) = match event {
            SqliteTelemetryEventV1::Migration {
                phase,
                schema_version,
            } => (
                EventDetailsV1::Migration {
                    phase: match phase {
                        SqliteMigrationPhaseV1::Started => MigrationPhaseV1::Started,
                        SqliteMigrationPhaseV1::Applied => MigrationPhaseV1::Applied,
                        SqliteMigrationPhaseV1::AlreadyCurrent => MigrationPhaseV1::AlreadyCurrent,
                        SqliteMigrationPhaseV1::Failed => MigrationPhaseV1::Failed,
                    },
                },
                match phase {
                    SqliteMigrationPhaseV1::Failed => ReasonCodeV1::StorageUnavailable,
                    SqliteMigrationPhaseV1::Started
                    | SqliteMigrationPhaseV1::Applied
                    | SqliteMigrationPhaseV1::AlreadyCurrent => ReasonCodeV1::Accepted,
                },
                vec![("schema_version".to_owned(), schema_version.to_string())],
            ),
            SqliteTelemetryEventV1::Recovery { phase } => (
                EventDetailsV1::Recovery {
                    phase: match phase {
                        SqliteRecoveryPhaseV1::Started => RecoveryPhaseV1::Started,
                        SqliteRecoveryPhaseV1::Completed => RecoveryPhaseV1::Installed,
                        SqliteRecoveryPhaseV1::Failed => RecoveryPhaseV1::Failed,
                    },
                },
                match phase {
                    SqliteRecoveryPhaseV1::Failed => ReasonCodeV1::RecoveryRequired,
                    SqliteRecoveryPhaseV1::Started | SqliteRecoveryPhaseV1::Completed => {
                        ReasonCodeV1::Accepted
                    }
                },
                Vec::new(),
            ),
            SqliteTelemetryEventV1::Integrity { status } => (
                EventDetailsV1::StorageDiagnostic {
                    kind: StorageDiagnosticKindV1::Integrity,
                },
                match status {
                    SqliteIntegrityStatusV1::Faulted | SqliteIntegrityStatusV1::Quarantined => {
                        ReasonCodeV1::IntegrityFailure
                    }
                },
                Vec::new(),
            ),
            SqliteTelemetryEventV1::StorageDiagnostic { kind } => (
                EventDetailsV1::StorageDiagnostic {
                    kind: match kind {
                        SqliteStorageDiagnosticKindV1::Connection => {
                            StorageDiagnosticKindV1::Connection
                        }
                        SqliteStorageDiagnosticKindV1::Integrity => {
                            StorageDiagnosticKindV1::Integrity
                        }
                        SqliteStorageDiagnosticKindV1::Query => StorageDiagnosticKindV1::Query,
                        SqliteStorageDiagnosticKindV1::Lock => StorageDiagnosticKindV1::Lock,
                    },
                },
                match kind {
                    SqliteStorageDiagnosticKindV1::Integrity => ReasonCodeV1::IntegrityFailure,
                    SqliteStorageDiagnosticKindV1::Connection
                    | SqliteStorageDiagnosticKindV1::Query => ReasonCodeV1::StorageUnavailable,
                    SqliteStorageDiagnosticKindV1::Lock => ReasonCodeV1::Busy,
                },
                Vec::new(),
            ),
        };
        let event_kind = details.event_kind();
        let _ = self.handle.submit(TelemetryDraftV1 {
            event: event_kind,
            reason,
            correlation: CorrelationV1::none(),
            adapter: Some(AdapterIdentityV1::Sqlite),
            details,
            observed_at_ms: None,
            attributes,
        });
    }
}

/// Adapts storage-owned `PostgreSQL` facts to the same bounded, non-blocking
/// telemetry queue used by the `SQLite` adapter. Delivery remains strictly
/// observational: the storage operation cannot observe queue or exporter
/// health and cannot depend on successful telemetry delivery.
#[derive(Clone)]
pub struct PostgresTelemetryBridge {
    handle: TelemetryHandle,
}

impl PostgresTelemetryBridge {
    #[must_use]
    pub fn new(handle: TelemetryHandle) -> Self {
        Self { handle }
    }
}

impl PostgresTelemetrySink for PostgresTelemetryBridge {
    fn emit(&self, event: PostgresTelemetryEventV1) {
        let (details, reason, attributes) = match event {
            PostgresTelemetryEventV1::Migration {
                phase,
                schema_version,
            } => (
                EventDetailsV1::Migration {
                    phase: match phase {
                        PostgresMigrationPhaseV1::Started => MigrationPhaseV1::Started,
                        PostgresMigrationPhaseV1::Applied => MigrationPhaseV1::Applied,
                        PostgresMigrationPhaseV1::AlreadyCurrent => {
                            MigrationPhaseV1::AlreadyCurrent
                        }
                        PostgresMigrationPhaseV1::Failed => MigrationPhaseV1::Failed,
                    },
                },
                match phase {
                    PostgresMigrationPhaseV1::Failed => ReasonCodeV1::StorageUnavailable,
                    PostgresMigrationPhaseV1::Started
                    | PostgresMigrationPhaseV1::Applied
                    | PostgresMigrationPhaseV1::AlreadyCurrent => ReasonCodeV1::Accepted,
                },
                vec![("schema_version".to_owned(), schema_version.to_string())],
            ),
            PostgresTelemetryEventV1::Recovery { phase } => (
                EventDetailsV1::Recovery {
                    phase: match phase {
                        PostgresRecoveryPhaseV1::Started => RecoveryPhaseV1::Started,
                        PostgresRecoveryPhaseV1::Completed => RecoveryPhaseV1::Installed,
                        PostgresRecoveryPhaseV1::Failed => RecoveryPhaseV1::Failed,
                    },
                },
                match phase {
                    PostgresRecoveryPhaseV1::Failed => ReasonCodeV1::RecoveryRequired,
                    PostgresRecoveryPhaseV1::Started | PostgresRecoveryPhaseV1::Completed => {
                        ReasonCodeV1::Accepted
                    }
                },
                Vec::new(),
            ),
            PostgresTelemetryEventV1::Integrity { status } => (
                EventDetailsV1::StorageDiagnostic {
                    kind: StorageDiagnosticKindV1::Integrity,
                },
                match status {
                    PostgresIntegrityStatusV1::Faulted | PostgresIntegrityStatusV1::Quarantined => {
                        ReasonCodeV1::IntegrityFailure
                    }
                },
                Vec::new(),
            ),
            PostgresTelemetryEventV1::StorageDiagnostic { kind } => (
                EventDetailsV1::StorageDiagnostic {
                    kind: match kind {
                        PostgresStorageDiagnosticKindV1::Connection => {
                            StorageDiagnosticKindV1::Connection
                        }
                        PostgresStorageDiagnosticKindV1::Integrity => {
                            StorageDiagnosticKindV1::Integrity
                        }
                        PostgresStorageDiagnosticKindV1::Query => StorageDiagnosticKindV1::Query,
                        PostgresStorageDiagnosticKindV1::Lock => StorageDiagnosticKindV1::Lock,
                    },
                },
                match kind {
                    PostgresStorageDiagnosticKindV1::Integrity => ReasonCodeV1::IntegrityFailure,
                    PostgresStorageDiagnosticKindV1::Connection
                    | PostgresStorageDiagnosticKindV1::Query => ReasonCodeV1::StorageUnavailable,
                    PostgresStorageDiagnosticKindV1::Lock => ReasonCodeV1::Busy,
                },
                Vec::new(),
            ),
        };
        let event_kind = details.event_kind();
        let _ = self.handle.submit(TelemetryDraftV1 {
            event: event_kind,
            reason,
            correlation: CorrelationV1::none(),
            adapter: Some(AdapterIdentityV1::Postgres),
            details,
            observed_at_ms: None,
            attributes,
        });
    }
}

/// Redacted, bounded event safe for JSON logs and exporter queues.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryEventV1 {
    pub schema: String,
    pub event: EventKindV1,
    pub reason: ReasonCodeV1,
    pub correlation: CorrelationV1,
    pub adapter: Option<AdapterIdentityV1>,
    pub details: EventDetailsV1,
    pub observed_at_ms: Option<u64>,
    pub attributes: BTreeMap<String, u64>,
}

impl TelemetryEventV1 {
    /// Deterministically removes unknown, textual, sensitive, duplicate, and
    /// oversized attributes. The typed details already exclude sensitive data.
    #[must_use]
    pub fn redact(draft: TelemetryDraftV1) -> Option<Self> {
        if draft.event != draft.details.event_kind() {
            return None;
        }
        let mut attributes = BTreeMap::new();
        for (key, value) in draft.attributes.into_iter().take(MAX_INPUT_ATTRIBUTES) {
            if !is_allowed_attribute(&key) {
                continue;
            }
            let Ok(value) = value.parse::<u64>() else {
                continue;
            };
            if value > MAX_ATTRIBUTE_VALUE {
                continue;
            }
            attributes
                .entry(key)
                .and_modify(|existing: &mut u64| *existing = (*existing).min(value))
                .or_insert(value);
        }
        let attributes = attributes.into_iter().take(MAX_ATTRIBUTES).collect();
        let event = Self {
            schema: TELEMETRY_SCHEMA_V1.to_owned(),
            event: draft.event,
            reason: draft.reason,
            correlation: draft.correlation,
            adapter: draft.adapter,
            details: draft.details,
            observed_at_ms: draft.observed_at_ms,
            attributes,
        };
        event.json_line().ok().map(|_| event)
    }

    fn validate_shape(&self) -> Result<(), JsonEncodingError> {
        if self.schema != TELEMETRY_SCHEMA_V1
            || self.event != self.details.event_kind()
            || self.attributes.len() > MAX_ATTRIBUTES
            || self
                .attributes
                .iter()
                .any(|(key, value)| !is_allowed_attribute(key) || *value > MAX_ATTRIBUTE_VALUE)
        {
            return Err(JsonEncodingError::InvalidEvent);
        }
        Ok(())
    }

    /// Serializes one bounded JSON log line.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails or the encoded line exceeds
    /// [`MAX_EVENT_BYTES`].
    pub fn json_line(&self) -> Result<String, JsonEncodingError> {
        self.validate_shape()?;
        let json = serde_json::to_string(self).map_err(JsonEncodingError::Serialization)?;
        if json.len() > MAX_EVENT_BYTES {
            return Err(JsonEncodingError::TooLarge);
        }
        Ok(json)
    }
}

fn is_allowed_attribute(key: &str) -> bool {
    matches!(
        key,
        "attempt"
            | "batch_size"
            | "bytes"
            | "duration_ms"
            | "queue_depth"
            | "retry_count"
            | "schema_version"
    )
}

/// JSON encoding errors are intentionally closed and do not contain payloads.
#[derive(Debug)]
pub enum JsonEncodingError {
    Serialization(serde_json::Error),
    TooLarge,
    InvalidEvent,
}

impl std::fmt::Display for JsonEncodingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Serialization(_) => "telemetry JSON serialization failed",
            Self::TooLarge => "telemetry event exceeds its size bound",
            Self::InvalidEvent => "telemetry event failed bounded redaction validation",
        })
    }
}

impl std::error::Error for JsonEncodingError {}

/// A single exporter failure class. It is safe to count and log.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportError {
    Unavailable,
    MalformedEndpoint,
    Serialization,
    InvalidEvent,
    BatchTooLarge,
    PayloadTooLarge,
}

/// Whether an exporter failure is safe to retry asynchronously.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportRetryClassV1 {
    RetryableOutage,
    PermanentConfiguration,
    PermanentPayload,
}

impl ExportError {
    #[must_use]
    pub const fn retry_class(self) -> ExportRetryClassV1 {
        match self {
            Self::Unavailable => ExportRetryClassV1::RetryableOutage,
            Self::MalformedEndpoint => ExportRetryClassV1::PermanentConfiguration,
            Self::Serialization
            | Self::InvalidEvent
            | Self::BatchTooLarge
            | Self::PayloadTooLarge => ExportRetryClassV1::PermanentPayload,
        }
    }
}

/// Vendor-neutral batch exporter seam.
pub trait TelemetryExporter: Send + Sync + 'static {
    /// Exports a bounded batch. Implementations must not be used before commit.
    ///
    /// # Errors
    ///
    /// Implementations return a closed [`ExportError`] when collection fails.
    fn export(&self, batch: &[TelemetryEventV1]) -> Result<(), ExportError>;
}

/// Transport seam for the optional OTLP-like exporter. No HTTP or vendor SDK
/// is pulled into the server crate.
pub trait OtlpTransport: Send + Sync + 'static {
    /// Sends one already-redacted OTLP-like JSON batch.
    ///
    /// # Errors
    ///
    /// Implementations return a closed [`ExportError`] when transport fails.
    fn send(&self, endpoint: &str, body: &str) -> Result<(), ExportError>;
}

/// A validated endpoint for an optional OTLP-like exporter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OtlpEndpointV1(String);

impl OtlpEndpointV1 {
    /// Validates and stores an endpoint without making a network request.
    ///
    /// # Errors
    ///
    /// Returns an error for an unsupported scheme, empty authority, embedded
    /// credentials, fragments, queries, whitespace, or an oversized value.
    pub fn parse(value: &str) -> Result<Self, EndpointError> {
        if value.is_empty()
            || value.len() > MAX_ENDPOINT_BYTES
            || value
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
            || !(value.starts_with("http://") || value.starts_with("https://"))
        {
            return Err(EndpointError::Malformed);
        }
        let authority = value
            .split_once("://")
            .and_then(|(_, rest)| {
                rest.split_once('/')
                    .map_or(Some(rest), |(host, _)| Some(host))
            })
            .ok_or(EndpointError::Malformed)?;
        if authority.is_empty()
            || authority.contains('@')
            || authority.contains('%')
            || authority.contains('\\')
            || value.contains('?')
            || value.contains('#')
        {
            return Err(EndpointError::Malformed);
        }
        validate_authority(authority)?;
        Ok(Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EndpointError {
    Malformed,
}

fn validate_authority(authority: &str) -> Result<(), EndpointError> {
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let (host, suffix) = rest.split_once(']').ok_or(EndpointError::Malformed)?;
        if host.is_empty() || !host.contains(':') {
            return Err(EndpointError::Malformed);
        }
        let port = match suffix {
            "" => None,
            suffix => Some(suffix.strip_prefix(':').ok_or(EndpointError::Malformed)?),
        };
        (host, port)
    } else if authority.matches(':').count() > 1 {
        return Err(EndpointError::Malformed);
    } else {
        authority
            .split_once(':')
            .map_or((authority, None), |(host, port)| (host, Some(port)))
    };

    if host.is_empty() || host.starts_with('.') || host.ends_with('.') {
        return Err(EndpointError::Malformed);
    }
    if let Some(port) = port {
        let parsed = port.parse::<u16>().map_err(|_| EndpointError::Malformed)?;
        if parsed == 0 {
            return Err(EndpointError::Malformed);
        }
    }
    Ok(())
}

/// OTLP/HTTP JSON logs exporter over an injected transport.
pub struct OtlpLikeExporter<T> {
    endpoint: OtlpEndpointV1,
    transport: T,
}

impl<T> OtlpLikeExporter<T> {
    /// Creates an exporter over a caller-supplied transport.
    ///
    /// # Errors
    ///
    /// Returns an error when `endpoint` fails validation.
    pub fn new(endpoint: &str, transport: T) -> Result<Self, EndpointError> {
        Ok(Self {
            endpoint: OtlpEndpointV1::parse(endpoint)?,
            transport,
        })
    }
}

impl<T: OtlpTransport> TelemetryExporter for OtlpLikeExporter<T> {
    fn export(&self, batch: &[TelemetryEventV1]) -> Result<(), ExportError> {
        if batch.len() > MAX_BATCH_SIZE {
            return Err(ExportError::BatchTooLarge);
        }
        if batch.iter().any(|event| event.json_line().is_err()) {
            return Err(ExportError::InvalidEvent);
        }
        let log_records: Vec<serde_json::Value> = batch
            .iter()
            .map(|event| {
                let body = event.json_line().unwrap_or_default();
                let timestamp = event
                    .observed_at_ms
                    .map_or_else(|| unix_ms().saturating_mul(1_000_000), |ms| ms.saturating_mul(1_000_000));
                serde_json::json!({
                    "timeUnixNano": timestamp.to_string(),
                    "severityText": "INFO",
                    "body": {"stringValue": body},
                    "attributes": [
                        {"key": "worldstream.schema", "value": {"stringValue": event.schema}},
                        {"key": "worldstream.event", "value": {"stringValue": event.event.label()}},
                        {"key": "worldstream.reason", "value": {"stringValue": reason_label(event.reason)}},
                    ],
                })
            })
            .collect();
        let body = serde_json::json!({
            "resourceLogs": [{
                "resource": {"attributes": [
                    {"key": "service.name", "value": {"stringValue": "worldstream"}},
                ]},
                "scopeLogs": [{
                    "scope": {"name": "worldstream.telemetry", "version": "1"},
                    "logRecords": log_records,
                }],
            }],
        });
        let body = serde_json::to_string(&body).map_err(|_| ExportError::Serialization)?;
        if body.len() > MAX_HTTP_REQUEST_BODY_BYTES {
            return Err(ExportError::PayloadTooLarge);
        }
        self.transport.send(self.endpoint.as_str(), &body)
    }
}

/// Plain HTTP transport for a local OTLP/HTTP collector.
#[derive(Clone, Copy, Debug, Default)]
pub struct StdHttpOtlpTransport;

impl OtlpTransport for StdHttpOtlpTransport {
    fn send(&self, endpoint: &str, body: &str) -> Result<(), ExportError> {
        let (scheme, authority, path) = parse_transport_endpoint(endpoint, body)?;
        if scheme != "http" {
            return Err(ExportError::MalformedEndpoint);
        }
        let mut stream = connect_stream(authority, 80)?;
        configure_stream(&stream)?;
        write_request(&mut stream, authority, path, body)?;
        stream
            .shutdown(Shutdown::Write)
            .map_err(|_| ExportError::Unavailable)?;
        read_response(&mut stream)
    }
}

/// HTTPS transport using rustls' certificate and hostname verification.
///
/// The default root store is the Mozilla `WebPKI` bundle. There is deliberately
/// no insecure verifier, plaintext fallback, endpoint credential support, or
/// caller-controlled trust toggle in the production constructor.
#[derive(Clone)]
pub struct TlsHttpOtlpTransport {
    config: Arc<ClientConfig>,
}

impl std::fmt::Debug for TlsHttpOtlpTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("TlsHttpOtlpTransport(verified)")
    }
}

impl Default for TlsHttpOtlpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl TlsHttpOtlpTransport {
    #[must_use]
    pub fn new() -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self::with_root_store(roots)
    }

    fn with_root_store(roots: RootCertStore) -> Self {
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Self {
            config: Arc::new(config),
        }
    }
}

impl OtlpTransport for TlsHttpOtlpTransport {
    fn send(&self, endpoint: &str, body: &str) -> Result<(), ExportError> {
        let (scheme, authority, path) = parse_transport_endpoint(endpoint, body)?;
        if scheme != "https" {
            return Err(ExportError::MalformedEndpoint);
        }
        let stream = connect_stream(authority, 443)?;
        configure_stream(&stream)?;
        let server_name = ServerName::try_from(authority_host(authority).to_owned())
            .map_err(|_| ExportError::Unavailable)?;
        let connection = ClientConnection::new(Arc::clone(&self.config), server_name)
            .map_err(|_| ExportError::Unavailable)?;
        let mut stream = StreamOwned::new(connection, stream);
        write_request(&mut stream, authority, path, body)?;
        stream.flush().map_err(|_| ExportError::Unavailable)?;
        stream
            .get_mut()
            .shutdown(Shutdown::Write)
            .map_err(|_| ExportError::Unavailable)?;
        read_response(&mut stream)
    }
}

fn parse_transport_endpoint<'a>(
    endpoint: &'a str,
    body: &str,
) -> Result<(&'a str, &'a str, &'a str), ExportError> {
    if body.len() > MAX_HTTP_REQUEST_BODY_BYTES {
        return Err(ExportError::PayloadTooLarge);
    }
    OtlpEndpointV1::parse(endpoint).map_err(|_| ExportError::MalformedEndpoint)?;
    split_http_endpoint(endpoint).ok_or(ExportError::MalformedEndpoint)
}

fn connect_stream(authority: &str, default_port: u16) -> Result<TcpStream, ExportError> {
    let addresses = resolve_addresses(authority, default_port)?;
    let deadline = Instant::now() + HTTP_CONNECT_TIMEOUT;
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(stream) = TcpStream::connect_timeout(&address, remaining) {
            return Ok(stream);
        }
    }
    Err(ExportError::Unavailable)
}

fn resolve_addresses(authority: &str, default_port: u16) -> Result<Vec<SocketAddr>, ExportError> {
    let connect_target = authority_connect_target(authority, default_port);
    DNS_RESOLVER_POOL
        .get_or_init(|| DnsResolverPool::new().ok())
        .as_ref()
        .ok_or(ExportError::Unavailable)?
        .resolve(connect_target, HTTP_CONNECT_TIMEOUT)
}

fn authority_connect_target(authority: &str, default_port: u16) -> String {
    let port = authority_port(authority).unwrap_or(default_port);
    if authority.starts_with('[') {
        authority.split_once(']').map_or_else(
            || authority.to_owned(),
            |(_, suffix)| {
                format!(
                    "{}:{}",
                    authority,
                    suffix.strip_prefix(':').unwrap_or(if default_port == 443 {
                        "443"
                    } else {
                        "80"
                    })
                )
            },
        )
    } else if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:{port}")
    }
}

fn authority_host(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map_or(rest, |(host, _)| host)
    } else {
        authority
            .split_once(':')
            .map_or(authority, |(host, _)| host)
    }
}

fn configure_stream(stream: &TcpStream) -> Result<(), ExportError> {
    stream
        .set_read_timeout(Some(HTTP_IO_TIMEOUT))
        .map_err(|_| ExportError::Unavailable)?;
    stream
        .set_write_timeout(Some(HTTP_IO_TIMEOUT))
        .map_err(|_| ExportError::Unavailable)
}

fn write_request<S: Write>(
    stream: &mut S,
    authority: &str,
    path: &str,
    body: &str,
) -> Result<(), ExportError> {
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|_| ExportError::Unavailable)
}

fn read_response<S: Read>(stream: &mut S) -> Result<(), ExportError> {
    let mut response = Vec::new();
    stream
        .take((MAX_HTTP_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response)
        .map_err(|_| ExportError::Unavailable)?;
    if response.len() > MAX_HTTP_RESPONSE_BYTES {
        return Err(ExportError::Unavailable);
    }
    let status = response
        .split(|byte| *byte == b' ')
        .nth(1)
        .and_then(|value| value.get(..3))
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(ExportError::Unavailable)?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(ExportError::Unavailable)
    }
}

fn split_http_endpoint(value: &str) -> Option<(&str, &str, &str)> {
    let (scheme, rest) = value.split_once("://")?;
    let (authority, path) = rest
        .find('/')
        .map_or((rest, "/"), |index| (&rest[..index], &rest[index..]));
    if authority.is_empty() {
        return None;
    }
    Some((scheme, authority, if path.is_empty() { "/" } else { path }))
}

fn authority_port(authority: &str) -> Option<u16> {
    if authority.starts_with('[') {
        authority
            .split_once(']')
            .and_then(|(_, suffix)| suffix.strip_prefix(':'))
            .and_then(|port| port.parse().ok())
    } else {
        authority
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse().ok())
    }
}

fn reason_label(reason: ReasonCodeV1) -> &'static str {
    match reason {
        ReasonCodeV1::Accepted => "accepted",
        ReasonCodeV1::Rejected => "rejected",
        ReasonCodeV1::Conflict => "conflict",
        ReasonCodeV1::Busy => "busy",
        ReasonCodeV1::Unauthorized => "unauthorized",
        ReasonCodeV1::Invalid => "invalid",
        ReasonCodeV1::StorageUnavailable => "storage_unavailable",
        ReasonCodeV1::CommitIndeterminate => "commit_indeterminate",
        ReasonCodeV1::Timeout => "timeout",
        ReasonCodeV1::RetryExhausted => "retry_exhausted",
        ReasonCodeV1::QueueFull => "queue_full",
        ReasonCodeV1::ExporterUnavailable => "exporter_unavailable",
        ReasonCodeV1::ExporterMalformedEndpoint => "exporter_malformed_endpoint",
        ReasonCodeV1::CollectorSlow => "collector_slow",
        ReasonCodeV1::MigrationMismatch => "migration_mismatch",
        ReasonCodeV1::SchemaMismatch => "schema_mismatch",
        ReasonCodeV1::IntegrityFailure => "integrity_failure",
        ReasonCodeV1::RecoveryRequired => "recovery_required",
        ReasonCodeV1::RecoveryComplete => "recovery_complete",
        ReasonCodeV1::RoomUnhealthy => "room_unhealthy",
        ReasonCodeV1::ProcessUnhealthy => "process_unhealthy",
    }
}

/// Fixed low-cardinality counters. Event families are labels from a closed
/// enum, never caller-provided identifiers.
#[derive(Clone, Default)]
pub struct TelemetryMetrics {
    inner: Arc<MetricCounters>,
}

struct MetricCounters {
    enqueued: AtomicU64,
    dropped: AtomicU64,
    overflow_warnings: AtomicU64,
    exporter_failures: AtomicU64,
    exporter_successes: AtomicU64,
    exporter_retryable_failures: AtomicU64,
    exporter_permanent_failures: AtomicU64,
    exporter_slow: AtomicU64,
    queued: AtomicU64,
    queue_high_water: AtomicU64,
    queue_completed: AtomicU64,
    queue_full: AtomicU64,
    events: [AtomicU64; 8],
    last_overflow_warning_ms: AtomicU64,
    overflow_warning_pending: AtomicBool,
}

impl Default for MetricCounters {
    fn default() -> Self {
        Self {
            enqueued: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            overflow_warnings: AtomicU64::new(0),
            exporter_failures: AtomicU64::new(0),
            exporter_successes: AtomicU64::new(0),
            exporter_retryable_failures: AtomicU64::new(0),
            exporter_permanent_failures: AtomicU64::new(0),
            exporter_slow: AtomicU64::new(0),
            queued: AtomicU64::new(0),
            queue_high_water: AtomicU64::new(0),
            queue_completed: AtomicU64::new(0),
            queue_full: AtomicU64::new(0),
            events: std::array::from_fn(|_| AtomicU64::new(0)),
            last_overflow_warning_ms: AtomicU64::new(0),
            overflow_warning_pending: AtomicBool::new(false),
        }
    }
}

impl TelemetryMetrics {
    #[must_use]
    pub fn snapshot(&self) -> MetricSnapshot {
        MetricSnapshot {
            enqueued: self.inner.enqueued.load(Ordering::Relaxed),
            dropped: self.inner.dropped.load(Ordering::Relaxed),
            overflow_warnings: self.inner.overflow_warnings.load(Ordering::Relaxed),
            exporter_failures: self.inner.exporter_failures.load(Ordering::Relaxed),
            exporter_successes: self.inner.exporter_successes.load(Ordering::Relaxed),
            exporter_retryable_failures: self
                .inner
                .exporter_retryable_failures
                .load(Ordering::Relaxed),
            exporter_permanent_failures: self
                .inner
                .exporter_permanent_failures
                .load(Ordering::Relaxed),
            exporter_slow: self.inner.exporter_slow.load(Ordering::Relaxed),
            queued: self.inner.queued.load(Ordering::Relaxed),
            events: EventKindV1::ALL
                .map(|kind| self.inner.events[kind.index()].load(Ordering::Relaxed)),
        }
    }

    pub(crate) fn queue_snapshot(&self, queue_capacity: usize) -> crate::InternalQueueSnapshot {
        let snapshot = self.snapshot();
        let process_current = usize::try_from(snapshot.queued).unwrap_or(usize::MAX);
        let process_high_water =
            usize::try_from(self.inner.queue_high_water.load(Ordering::Acquire))
                .unwrap_or(usize::MAX);
        crate::InternalQueueSnapshot {
            name: "telemetry_exporter",
            capacity_scope: "global",
            capacity: queue_capacity,
            process_current,
            process_high_water,
            unit_high_water: process_high_water,
            activity_total: snapshot.enqueued,
            completion_total: self.inner.queue_completed.load(Ordering::Relaxed),
            backpressure_total: self.inner.queue_full.load(Ordering::Relaxed),
        }
    }

    /// Renders valid Prometheus exposition text with fixed names and labels.
    #[must_use]
    pub fn prometheus_text(&self) -> String {
        self.prometheus_text_with_queue_capacity(None)
    }

    fn prometheus_text_with_queue_capacity(&self, queue_capacity: Option<usize>) -> String {
        let snapshot = self.snapshot();
        let mut text = String::new();
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_enqueued_total counter\nworldstream_telemetry_enqueued_total {}",
            snapshot.enqueued
        );
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_dropped_total counter\nworldstream_telemetry_dropped_total {}",
            snapshot.dropped
        );
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_overflow_warnings_total counter\nworldstream_telemetry_overflow_warnings_total {}",
            snapshot.overflow_warnings
        );
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_exporter_failures_total counter\nworldstream_telemetry_exporter_failures_total {}",
            snapshot.exporter_failures
        );
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_exporter_retryable_failures_total counter\nworldstream_telemetry_exporter_retryable_failures_total {}",
            snapshot.exporter_retryable_failures
        );
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_exporter_successes_total counter\nworldstream_telemetry_exporter_successes_total {}",
            snapshot.exporter_successes
        );
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_exporter_permanent_failures_total counter\nworldstream_telemetry_exporter_permanent_failures_total {}",
            snapshot.exporter_permanent_failures
        );
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_exporter_slow_total counter\nworldstream_telemetry_exporter_slow_total {}",
            snapshot.exporter_slow
        );
        let _ = writeln!(
            text,
            "# TYPE worldstream_telemetry_queued gauge\nworldstream_telemetry_queued {}",
            snapshot.queued
        );
        if let Some(queue_capacity) = queue_capacity {
            let _ = writeln!(
                text,
                "# TYPE worldstream_telemetry_queue_capacity gauge\nworldstream_telemetry_queue_capacity {queue_capacity}"
            );
        }
        let _ = writeln!(text, "# TYPE worldstream_telemetry_events_total counter");
        for (kind, value) in EventKindV1::ALL.into_iter().zip(snapshot.events) {
            let _ = writeln!(
                text,
                "worldstream_telemetry_events_total{{event=\"{}\"}} {}",
                kind.label(),
                value
            );
        }
        text
    }

    fn note_drop(&self) {
        self.inner.dropped.fetch_add(1, Ordering::Relaxed);
    }

    fn note_overflow(&self) {
        self.note_drop();
        self.inner.queue_full.fetch_add(1, Ordering::Relaxed);
        let now = unix_ms();
        let previous = self.inner.last_overflow_warning_ms.load(Ordering::Relaxed);
        let warning_interval =
            u64::try_from(OVERFLOW_WARNING_INTERVAL.as_millis()).unwrap_or(u64::MAX);
        if now >= previous.saturating_add(warning_interval)
            && self
                .inner
                .last_overflow_warning_ms
                .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            self.inner.overflow_warnings.fetch_add(1, Ordering::Relaxed);
            // Warning emission is deferred to the worker. The producer may run
            // on the commit/response path, so it must never call a potentially
            // blocking tracing subscriber while reporting queue saturation.
            self.inner
                .overflow_warning_pending
                .store(true, Ordering::Release);
        }
    }

    fn emit_pending_overflow_warning(&self) {
        if self
            .inner
            .overflow_warning_pending
            .swap(false, Ordering::AcqRel)
        {
            tracing::warn!("telemetry queue full; dropping telemetry");
        }
    }
}

/// A stable view of all telemetry counters.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetricSnapshot {
    pub enqueued: u64,
    pub dropped: u64,
    pub overflow_warnings: u64,
    pub exporter_failures: u64,
    pub exporter_successes: u64,
    pub exporter_retryable_failures: u64,
    pub exporter_permanent_failures: u64,
    pub exporter_slow: u64,
    pub queued: u64,
    pub events: [u64; 8],
}

/// Exporter health is diagnostic only; it never gates the authoritative path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExporterHealthV1 {
    Healthy,
    Degraded,
    Unavailable,
    Stopped,
}

/// Bounded runtime health view suitable for a readiness/health endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TelemetryHealthSnapshotV1 {
    pub queue_capacity: usize,
    pub queue_depth: u64,
    pub dropped: u64,
    pub exporter: ExporterHealthV1,
    pub exporter_successes: u64,
    pub exporter_failures: u64,
    pub retryable_failures: u64,
    pub permanent_failures: u64,
}

impl TelemetryMetrics {
    #[must_use]
    pub fn health_snapshot(
        &self,
        queue_capacity: usize,
        stopped: bool,
    ) -> TelemetryHealthSnapshotV1 {
        let metrics = self.snapshot();
        let exporter = if stopped {
            ExporterHealthV1::Stopped
        } else if metrics.exporter_retryable_failures > 0 && metrics.exporter_successes == 0 {
            ExporterHealthV1::Unavailable
        } else if metrics.exporter_failures > 0 {
            ExporterHealthV1::Degraded
        } else {
            ExporterHealthV1::Healthy
        };
        TelemetryHealthSnapshotV1 {
            queue_capacity,
            queue_depth: metrics.queued.min(queue_capacity as u64),
            dropped: metrics.dropped,
            exporter,
            exporter_successes: metrics.exporter_successes,
            exporter_failures: metrics.exporter_failures,
            retryable_failures: metrics.exporter_retryable_failures,
            permanent_failures: metrics.exporter_permanent_failures,
        }
    }
}

/// Runtime bounds for the background exporter worker.
#[derive(Clone, Copy, Debug)]
pub struct TelemetryConfig {
    pub queue_capacity: usize,
    pub batch_size: usize,
    pub slow_export_after: Duration,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            queue_capacity: DEFAULT_QUEUE_CAPACITY,
            batch_size: 32,
            slow_export_after: Duration::from_millis(250),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueSubmitResult {
    Enqueued,
    Dropped,
}

/// Cloneable, nonblocking producer handle. `try_send` is the only queue
/// operation exposed to callers.
#[derive(Clone)]
pub struct TelemetryHandle {
    sender: SyncSender<TelemetryEventV1>,
    metrics: TelemetryMetrics,
    stopped: Arc<AtomicBool>,
    queue_capacity: usize,
}

impl TelemetryHandle {
    /// Renders the current bounded metrics snapshot for the gateway metrics
    /// endpoint. This is a read-only diagnostic operation and does not probe
    /// or depend on the exporter.
    #[must_use]
    pub fn prometheus_text(&self) -> String {
        self.metrics
            .prometheus_text_with_queue_capacity(Some(self.queue_capacity))
    }

    pub(crate) fn queue_snapshot(&self) -> crate::InternalQueueSnapshot {
        self.metrics.queue_snapshot(self.queue_capacity)
    }

    #[must_use]
    pub fn submit(&self, draft: TelemetryDraftV1) -> QueueSubmitResult {
        let Some(event) = TelemetryEventV1::redact(draft) else {
            self.metrics.note_drop();
            return QueueSubmitResult::Dropped;
        };
        self.submit_event(event)
    }

    #[must_use]
    pub fn submit_event(&self, event: TelemetryEventV1) -> QueueSubmitResult {
        if self.stopped.load(Ordering::Acquire) || event.json_line().is_err() {
            self.metrics.note_drop();
            return QueueSubmitResult::Dropped;
        }
        let kind = event.event;
        let capacity = u64::try_from(self.queue_capacity).unwrap_or(u64::MAX);
        let mut current = self.metrics.inner.queued.load(Ordering::Acquire);
        let queued_depth = loop {
            let Some(next) = current.checked_add(1).filter(|next| *next <= capacity) else {
                self.metrics.note_overflow();
                return QueueSubmitResult::Dropped;
            };
            match self.metrics.inner.queued.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break next,
                Err(observed) => current = observed,
            }
        };
        match self.sender.try_send(event) {
            Ok(()) => {
                atomic_max_u64(&self.metrics.inner.queue_high_water, queued_depth);
                self.metrics.inner.enqueued.fetch_add(1, Ordering::Relaxed);
                self.metrics.inner.events[kind.index()].fetch_add(1, Ordering::Relaxed);
                QueueSubmitResult::Enqueued
            }
            Err(TrySendError::Full(_)) => {
                self.metrics.inner.queued.fetch_sub(1, Ordering::Relaxed);
                self.metrics.note_overflow();
                QueueSubmitResult::Dropped
            }
            Err(TrySendError::Disconnected(_)) => {
                self.metrics.inner.queued.fetch_sub(1, Ordering::Relaxed);
                self.metrics.note_drop();
                QueueSubmitResult::Dropped
            }
        }
    }
}

/// Result of bounded shutdown flushing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlushOutcome {
    Flushed,
    TimedOut,
}

/// Owns one bounded exporter worker. Export failures are counted and discarded
/// so they cannot change an authoritative operation result.
pub struct TelemetryRuntime {
    handle: TelemetryHandle,
    queue_capacity: usize,
    stopped: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl TelemetryRuntime {
    /// Starts one bounded worker over a vendor-neutral exporter.
    ///
    /// # Errors
    ///
    /// Returns an error when queue/batch bounds are invalid or the worker
    /// thread cannot be created.
    pub fn new(
        config: TelemetryConfig,
        exporter: Arc<dyn TelemetryExporter>,
    ) -> Result<Self, RuntimeConfigError> {
        if config.queue_capacity == 0
            || config.queue_capacity > MAX_QUEUE_CAPACITY
            || config.batch_size == 0
            || config.batch_size > MAX_BATCH_SIZE
        {
            return Err(RuntimeConfigError::InvalidBounds);
        }
        let (sender, receiver) = mpsc::sync_channel(config.queue_capacity);
        let metrics = TelemetryMetrics::default();
        let stopped = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let worker_stopped = Arc::clone(&stopped);
        let worker_finished = Arc::clone(&finished);
        let worker_metrics = metrics.clone();
        let worker_config = config;
        let worker = thread::Builder::new()
            .name("worldstream-telemetry".to_owned())
            .spawn(move || {
                worker_loop(
                    receiver,
                    exporter,
                    worker_metrics,
                    worker_stopped,
                    worker_finished,
                    worker_config,
                );
            })
            .map_err(|_| RuntimeConfigError::WorkerUnavailable)?;
        let handle = TelemetryHandle {
            sender,
            metrics,
            stopped: Arc::clone(&stopped),
            queue_capacity: config.queue_capacity,
        };
        Ok(Self {
            handle,
            queue_capacity: config.queue_capacity,
            stopped,
            finished,
            worker: Some(worker),
        })
    }

    #[must_use]
    pub fn handle(&self) -> TelemetryHandle {
        self.handle.clone()
    }

    #[must_use]
    pub fn metrics(&self) -> TelemetryMetrics {
        self.handle.metrics.clone()
    }

    /// Returns bounded exporter and queue health without probing the network.
    #[must_use]
    pub fn health_snapshot(&self) -> TelemetryHealthSnapshotV1 {
        self.handle
            .metrics
            .health_snapshot(self.queue_capacity, self.stopped.load(Ordering::Acquire))
    }

    #[must_use]
    pub fn shutdown(mut self, timeout: Duration) -> FlushOutcome {
        self.stopped.store(true, Ordering::Release);
        let timeout = timeout.min(MAX_SHUTDOWN_FLUSH);
        let deadline = Instant::now() + timeout;
        while !self.finished.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(1));
        }
        if self.finished.load(Ordering::Acquire) {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            FlushOutcome::Flushed
        } else {
            let _ = self.worker.take();
            FlushOutcome::TimedOut
        }
    }
}

impl Drop for TelemetryRuntime {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

#[allow(clippy::needless_pass_by_value)]
fn worker_loop(
    receiver: Receiver<TelemetryEventV1>,
    exporter: Arc<dyn TelemetryExporter>,
    metrics: TelemetryMetrics,
    stopped: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    config: TelemetryConfig,
) {
    loop {
        metrics.emit_pending_overflow_warning();
        let first = if stopped.load(Ordering::Acquire) {
            match receiver.try_recv() {
                Ok(event) => event,
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        } else {
            match receiver.recv_timeout(Duration::from_millis(25)) {
                Ok(event) => event,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        };
        let mut batch = Vec::with_capacity(config.batch_size);
        batch.push(first);
        while batch.len() < config.batch_size {
            match receiver.try_recv() {
                Ok(event) => batch.push(event),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        metrics
            .inner
            .queued
            .fetch_sub(batch.len() as u64, Ordering::Relaxed);
        metrics
            .inner
            .queue_completed
            .fetch_add(batch.len() as u64, Ordering::Relaxed);
        let started = Instant::now();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| exporter.export(&batch)))
                .unwrap_or(Err(ExportError::Unavailable));
        match result {
            Ok(()) => {
                metrics
                    .inner
                    .exporter_successes
                    .fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                metrics
                    .inner
                    .exporter_failures
                    .fetch_add(1, Ordering::Relaxed);
                match error.retry_class() {
                    ExportRetryClassV1::RetryableOutage => {
                        metrics
                            .inner
                            .exporter_retryable_failures
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    ExportRetryClassV1::PermanentConfiguration
                    | ExportRetryClassV1::PermanentPayload => {
                        metrics
                            .inner
                            .exporter_permanent_failures
                            .fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }
        if started.elapsed() >= config.slow_export_after {
            metrics.inner.exporter_slow.fetch_add(1, Ordering::Relaxed);
        }
    }
    metrics.emit_pending_overflow_warning();
    finished.store(true, Ordering::Release);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeConfigError {
    InvalidBounds,
    WorkerUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaReadinessV1 {
    Current,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageReadinessV1 {
    HealthyWritable,
    HealthyReadOnly,
    Unhealthy,
}

/// Process-vs-room readiness facts. No room identifier is accepted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadinessInputV1 {
    pub schema: SchemaReadinessV1,
    pub storage: StorageReadinessV1,
    pub writer_running: bool,
    pub scheduler_running: bool,
    pub unhealthy_rooms: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessReasonV1 {
    SchemaUnavailable,
    StorageUnhealthy,
    StorageReadOnly,
    WriterStopped,
    SchedulerStopped,
    RecoveryRequired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessClassificationV1 {
    Ready,
    RoomUnhealthy { count: u32 },
    ProcessUnhealthy { reason: ReadinessReasonV1 },
}

/// Recovery facts are global process state, unlike a single unhealthy Room.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecoveryReadinessV1 {
    Complete,
    Required,
}

/// Stable, redacted action metadata for readiness responses and logs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadinessDiagnosticV1 {
    pub code: &'static str,
    pub action: &'static str,
}

impl ReadinessClassificationV1 {
    /// Returns an actionable diagnostic containing no Room, user, or storage
    /// identifiers.
    #[must_use]
    pub const fn diagnostic(self) -> ReadinessDiagnosticV1 {
        match self {
            Self::Ready => ReadinessDiagnosticV1 {
                code: "ready",
                action: "no action required",
            },
            Self::RoomUnhealthy { .. } => ReadinessDiagnosticV1 {
                code: "room_unhealthy",
                action: "inspect Room-scoped diagnostics; process readiness remains available",
            },
            Self::ProcessUnhealthy { reason } => match reason {
                ReadinessReasonV1::SchemaUnavailable => ReadinessDiagnosticV1 {
                    code: "schema_unavailable",
                    action: "restore the versioned schema before serving mutations",
                },
                ReadinessReasonV1::StorageUnhealthy => ReadinessDiagnosticV1 {
                    code: "storage_unhealthy",
                    action: "restore writable durable storage before serving mutations",
                },
                ReadinessReasonV1::StorageReadOnly => ReadinessDiagnosticV1 {
                    code: "storage_read_only",
                    action: "restore writable durable storage before serving mutations",
                },
                ReadinessReasonV1::WriterStopped => ReadinessDiagnosticV1 {
                    code: "writer_stopped",
                    action: "restart the durable writer before serving mutations",
                },
                ReadinessReasonV1::SchedulerStopped => ReadinessDiagnosticV1 {
                    code: "scheduler_stopped",
                    action: "start the scheduler before claiming readiness",
                },
                ReadinessReasonV1::RecoveryRequired => ReadinessDiagnosticV1 {
                    code: "recovery_required",
                    action: "complete durable recovery before claiming readiness",
                },
            },
        }
    }
}

/// A Room failure is local; global schema/storage/runtime failures are process
/// failures. Telemetry/exporter health is intentionally not an input.
#[must_use]
pub fn classify_readiness(input: ReadinessInputV1) -> ReadinessClassificationV1 {
    classify_readiness_with_recovery(input, RecoveryReadinessV1::Complete)
}

/// Classifies readiness while keeping recovery failures global and Room
/// failures local. Telemetry/exporter health is intentionally not an input.
#[must_use]
pub fn classify_readiness_with_recovery(
    input: ReadinessInputV1,
    recovery: RecoveryReadinessV1,
) -> ReadinessClassificationV1 {
    let process_reason = if input.schema == SchemaReadinessV1::Unavailable {
        Some(ReadinessReasonV1::SchemaUnavailable)
    } else if input.storage == StorageReadinessV1::Unhealthy {
        Some(ReadinessReasonV1::StorageUnhealthy)
    } else if input.storage == StorageReadinessV1::HealthyReadOnly {
        Some(ReadinessReasonV1::StorageReadOnly)
    } else if !input.writer_running {
        Some(ReadinessReasonV1::WriterStopped)
    } else if !input.scheduler_running {
        Some(ReadinessReasonV1::SchedulerStopped)
    } else if recovery == RecoveryReadinessV1::Required {
        Some(ReadinessReasonV1::RecoveryRequired)
    } else {
        None
    };
    if let Some(reason) = process_reason {
        ReadinessClassificationV1::ProcessUnhealthy { reason }
    } else if input.unhealthy_rooms == 0 {
        ReadinessClassificationV1::Ready
    } else {
        ReadinessClassificationV1::RoomUnhealthy {
            count: input.unhealthy_rooms,
        }
    }
}

fn decode_hex<const N: usize>(value: &str) -> Result<[u8; N], TraceParentError> {
    if value.len() != N * 2
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() || byte.is_ascii_uppercase())
    {
        return Err(TraceParentError::Malformed);
    }
    let mut output = [0; N];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_digit(pair[0])? << 4) | hex_digit(pair[1])?;
    }
    Ok(output)
}

fn decode_hex_byte(value: &str) -> Result<u8, TraceParentError> {
    Ok(decode_hex::<1>(value)?[0])
}

fn hex_digit(value: u8) -> Result<u8, TraceParentError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(TraceParentError::Malformed),
    }
}

fn hex<const N: usize>(bytes: [u8; N]) -> String {
    let mut output = String::with_capacity(N * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            u64::try_from(duration.as_millis().min(u128::from(u64::MAX))).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn sensitive_dsn_fixture() -> String {
        ["postgres://user:", "password", "@db/app"].concat()
    }

    fn draft(details: EventDetailsV1) -> TelemetryDraftV1 {
        TelemetryDraftV1 {
            event: details.event_kind(),
            reason: ReasonCodeV1::Accepted,
            correlation: CorrelationV1::none(),
            adapter: None,
            details,
            observed_at_ms: Some(42),
            attributes: vec![
                ("duration_ms".to_owned(), "7".to_owned()),
                ("payload".to_owned(), "private clue".to_owned()),
                ("capability".to_owned(), "secret".to_owned()),
                (
                    "authorization".to_owned(),
                    "Bearer very-secret-token".to_owned(),
                ),
                ("dsn".to_owned(), sensitive_dsn_fixture()),
                ("password".to_owned(), "secret-password".to_owned()),
                ("bytes".to_owned(), "999999999999999999999".to_owned()),
            ],
        }
    }

    #[test]
    fn redaction_is_deterministic_and_excludes_sensitive_or_high_cardinality_values() {
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::CommitOutcome {
            outcome: CommitOutcomeV1::Committed,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(event.attributes.get("duration_ms"), Some(&7));
        assert!(
            !event
                .json_line()
                .unwrap_or_default()
                .contains("private clue")
        );
        assert!(!event.json_line().unwrap_or_default().contains("capability"));
        assert!(!event.json_line().unwrap_or_default().contains("secret"));
        assert!(
            !event
                .json_line()
                .unwrap_or_default()
                .contains("Bearer very-secret-token")
        );
        assert!(
            !event
                .json_line()
                .unwrap_or_default()
                .contains(&sensitive_dsn_fixture())
        );
        assert!(
            !event
                .json_line()
                .unwrap_or_default()
                .contains("secret-password")
        );
        assert!(
            !event
                .json_line()
                .unwrap_or_default()
                .contains("999999999999999999999")
        );
        let again = TelemetryEventV1::redact(draft(EventDetailsV1::CommitOutcome {
            outcome: CommitOutcomeV1::Committed,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(
            event.json_line().unwrap_or_default(),
            again.json_line().unwrap_or_default()
        );
    }

    #[test]
    fn traceparent_parses_and_propagates_as_w3c_header() {
        let parent =
            TraceParentV1::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
                .unwrap_or_else(|error| unreachable!("valid traceparent: {error}"));
        assert_eq!(
            parent.to_header(),
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        );
        let child = parent
            .child([1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap_or_else(|_| unreachable!("nonzero span"));
        assert!(child.to_header().ends_with("-0102030405060708-01"));
        assert_eq!(
            TraceParentV1::parse("00-00000000000000000000000000000000-00f067aa0ba902b7-01"),
            Err(TraceParentError::ZeroIdentifier)
        );
        assert_eq!(
            TraceParentV1::parse("00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01"),
            Err(TraceParentError::Malformed)
        );
        assert_eq!(
            TraceParentV1::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra"),
            Err(TraceParentError::Malformed)
        );
        assert_eq!(
            TraceParentV1::parse("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0100"),
            Err(TraceParentError::Malformed)
        );
        assert_eq!(
            TraceParentV1::parse("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01 "),
            Err(TraceParentError::Malformed)
        );
    }

    #[test]
    fn metrics_are_prometheus_text_with_only_fixed_labels() {
        let metrics = TelemetryMetrics::default();
        metrics.inner.enqueued.store(2, Ordering::Relaxed);
        metrics.inner.exporter_successes.store(3, Ordering::Relaxed);
        metrics
            .inner
            .exporter_permanent_failures
            .store(4, Ordering::Relaxed);
        metrics.inner.queued.store(5, Ordering::Relaxed);
        metrics.inner.events[EventKindV1::Admission.index()].store(2, Ordering::Relaxed);
        let text = metrics.prometheus_text();
        assert!(text.contains("worldstream_telemetry_events_total{event=\"admission\"} 2"));
        assert!(text.contains("worldstream_telemetry_exporter_successes_total 3"));
        assert!(text.contains("worldstream_telemetry_exporter_permanent_failures_total 4"));
        assert!(text.contains("worldstream_telemetry_queued 5"));
        assert!(!text.contains("room_id"));
        assert!(!text.contains("private"));
    }

    #[test]
    fn event_and_reason_wire_vocabulary_is_closed_and_versioned() {
        let details = [
            EventDetailsV1::Admission {
                outcome: AdmissionOutcomeV1::Accepted,
            },
            EventDetailsV1::CommitOutcome {
                outcome: CommitOutcomeV1::Committed,
            },
            EventDetailsV1::Timer {
                phase: TimerPhaseV1::Fired,
            },
            EventDetailsV1::FrameDelivery {
                outcome: FrameDeliveryOutcomeV1::Delivered,
            },
            EventDetailsV1::Activation {
                phase: ActivationPhaseV1::Claimed,
            },
            EventDetailsV1::Recovery {
                phase: RecoveryPhaseV1::Replayed,
            },
            EventDetailsV1::Migration {
                phase: MigrationPhaseV1::Applied,
            },
            EventDetailsV1::StorageDiagnostic {
                kind: StorageDiagnosticKindV1::Connection,
            },
        ];
        for detail in details {
            let event = TelemetryEventV1::redact(draft(detail))
                .unwrap_or_else(|| unreachable!("typed event detail matches its family"));
            let wire = event
                .json_line()
                .unwrap_or_else(|error| unreachable!("bounded telemetry event: {error}"));
            let value: serde_json::Value = serde_json::from_str(&wire)
                .unwrap_or_else(|error| unreachable!("valid telemetry JSON: {error}"));
            assert_eq!(value["schema"], TELEMETRY_SCHEMA_V1);
            assert_eq!(
                value["event"],
                serde_json::to_value(detail.event_kind())
                    .unwrap_or_else(|error| unreachable!("closed event vocabulary: {error}"))
            );
            assert_eq!(
                value["details"]["kind"],
                serde_json::to_value(detail.event_kind())
                    .unwrap_or_else(|error| unreachable!("closed event vocabulary: {error}"))
            );
            assert!(!wire.contains("room_id"));
            assert!(!wire.contains("payload"));
            assert!(wire.len() <= MAX_EVENT_BYTES);
        }

        let reasons = [
            (ReasonCodeV1::Accepted, "accepted"),
            (ReasonCodeV1::Rejected, "rejected"),
            (ReasonCodeV1::Conflict, "conflict"),
            (ReasonCodeV1::Busy, "busy"),
            (ReasonCodeV1::Unauthorized, "unauthorized"),
            (ReasonCodeV1::Invalid, "invalid"),
            (ReasonCodeV1::StorageUnavailable, "storage_unavailable"),
            (ReasonCodeV1::CommitIndeterminate, "commit_indeterminate"),
            (ReasonCodeV1::Timeout, "timeout"),
            (ReasonCodeV1::RetryExhausted, "retry_exhausted"),
            (ReasonCodeV1::QueueFull, "queue_full"),
            (ReasonCodeV1::ExporterUnavailable, "exporter_unavailable"),
            (
                ReasonCodeV1::ExporterMalformedEndpoint,
                "exporter_malformed_endpoint",
            ),
            (ReasonCodeV1::CollectorSlow, "collector_slow"),
            (ReasonCodeV1::MigrationMismatch, "migration_mismatch"),
            (ReasonCodeV1::SchemaMismatch, "schema_mismatch"),
            (ReasonCodeV1::IntegrityFailure, "integrity_failure"),
            (ReasonCodeV1::RecoveryRequired, "recovery_required"),
            (ReasonCodeV1::RecoveryComplete, "recovery_complete"),
            (ReasonCodeV1::RoomUnhealthy, "room_unhealthy"),
            (ReasonCodeV1::ProcessUnhealthy, "process_unhealthy"),
        ];
        for (reason, expected) in reasons {
            assert_eq!(
                serde_json::to_string(&reason)
                    .unwrap_or_else(|error| unreachable!("closed reason vocabulary: {error}")),
                format!("\"{expected}\"")
            );
        }
    }

    #[test]
    fn adapter_identity_is_closed_optional_and_backend_neutral() {
        for adapter in [AdapterIdentityV1::Sqlite, AdapterIdentityV1::Postgres] {
            let mut event_draft = draft(EventDetailsV1::StorageDiagnostic {
                kind: StorageDiagnosticKindV1::Connection,
            });
            event_draft.adapter = Some(adapter);
            let event = TelemetryEventV1::redact(event_draft)
                .unwrap_or_else(|| unreachable!("matching typed event detail"));
            let wire = event
                .json_line()
                .unwrap_or_else(|error| unreachable!("bounded telemetry event: {error}"));
            assert!(wire.contains(&format!(
                "\"adapter\":\"{}\"",
                match adapter {
                    AdapterIdentityV1::Sqlite => "sqlite",
                    AdapterIdentityV1::Postgres => "postgres",
                }
            )));
            assert!(!wire.contains("room_id"));
            assert!(!wire.contains("projection"));
        }
    }

    #[test]
    fn dimensions_are_allowlisted_bounded_and_duplicate_deterministic() {
        let mut attributes = vec![
            ("duration_ms".to_owned(), "9".to_owned()),
            ("duration_ms".to_owned(), "3".to_owned()),
            ("room_id".to_owned(), "42".to_owned()),
            ("bytes".to_owned(), "1000000001".to_owned()),
        ];
        attributes.extend((0..32).map(|index| (format!("unknown_{index}"), "1".to_owned())));
        let mut event_draft = draft(EventDetailsV1::Admission {
            outcome: AdmissionOutcomeV1::Accepted,
        });
        event_draft.attributes = attributes;
        let event = TelemetryEventV1::redact(event_draft)
            .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(
            event.attributes,
            BTreeMap::from([(String::from("duration_ms"), 3)])
        );
        assert!(event.attributes.len() <= MAX_ATTRIBUTES);
        assert!(event.json_line().is_ok());
    }

    #[test]
    fn readiness_keeps_room_failure_local_but_promotes_global_failures() {
        let base = ReadinessInputV1 {
            schema: SchemaReadinessV1::Current,
            storage: StorageReadinessV1::HealthyWritable,
            writer_running: true,
            scheduler_running: true,
            unhealthy_rooms: 1,
        };
        assert_eq!(
            classify_readiness(base),
            ReadinessClassificationV1::RoomUnhealthy { count: 1 }
        );
        assert_eq!(
            classify_readiness(ReadinessInputV1 {
                storage: StorageReadinessV1::Unhealthy,
                ..base
            }),
            ReadinessClassificationV1::ProcessUnhealthy {
                reason: ReadinessReasonV1::StorageUnhealthy
            }
        );
        assert_eq!(
            classify_readiness(ReadinessInputV1 {
                schema: SchemaReadinessV1::Unavailable,
                ..base
            }),
            ReadinessClassificationV1::ProcessUnhealthy {
                reason: ReadinessReasonV1::SchemaUnavailable
            }
        );
        assert_eq!(
            classify_readiness_with_recovery(base, RecoveryReadinessV1::Required),
            ReadinessClassificationV1::ProcessUnhealthy {
                reason: ReadinessReasonV1::RecoveryRequired
            }
        );
        let diagnostic = ReadinessClassificationV1::RoomUnhealthy { count: 2 }.diagnostic();
        assert_eq!(diagnostic.code, "room_unhealthy");
        assert!(diagnostic.action.contains("Room-scoped"));
        assert!(!diagnostic.action.contains("room_id"));
        assert_eq!(
            (ReadinessClassificationV1::ProcessUnhealthy {
                reason: ReadinessReasonV1::RecoveryRequired
            })
            .diagnostic()
            .code,
            "recovery_required"
        );
    }

    struct RecordingTransport {
        calls: Mutex<Vec<String>>,
        failure: bool,
    }

    impl OtlpTransport for RecordingTransport {
        fn send(&self, endpoint: &str, body: &str) -> Result<(), ExportError> {
            if self.failure {
                return Err(ExportError::Unavailable);
            }
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("{endpoint}:{body}"));
            Ok(())
        }
    }

    #[test]
    fn otlp_like_exporter_rejects_malformed_endpoints_and_supports_outage_seam() {
        assert_eq!(
            OtlpEndpointV1::parse("ftp://collector"),
            Err(EndpointError::Malformed)
        );
        assert!(OtlpEndpointV1::parse("https://user:password@collector").is_err());
        assert!(OtlpEndpointV1::parse("https://collector/path?token=secret").is_err());
        assert!(OtlpEndpointV1::parse("https://collector/path#fragment").is_err());
        assert!(OtlpEndpointV1::parse("https://collector:").is_err());
        assert!(OtlpEndpointV1::parse("https://collector:0").is_err());
        assert!(OtlpEndpointV1::parse("https://collector:65536").is_err());
        assert!(OtlpEndpointV1::parse("https://[::1]:4318").is_ok());
        assert!(OtlpEndpointV1::parse("https://::1:4318").is_err());
        let transport = RecordingTransport {
            calls: Mutex::new(Vec::new()),
            failure: true,
        };
        let exporter = OtlpLikeExporter::new("http://collector:4318", transport)
            .unwrap_or_else(|_| unreachable!("valid endpoint"));
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::Recovery {
            phase: RecoveryPhaseV1::Started,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(exporter.export(&[event]), Err(ExportError::Unavailable));
    }

    #[test]
    fn otlp_like_exporter_rejects_unbounded_batches_before_transport() {
        let transport = RecordingTransport {
            calls: Mutex::new(Vec::new()),
            failure: false,
        };
        let exporter = OtlpLikeExporter::new("http://collector:4318", transport)
            .unwrap_or_else(|_| unreachable!("valid endpoint"));
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::Recovery {
            phase: RecoveryPhaseV1::Started,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        let batch = vec![event; MAX_BATCH_SIZE + 1];
        assert_eq!(exporter.export(&batch), Err(ExportError::BatchTooLarge));
    }

    #[test]
    fn http_transports_reject_oversized_bodies_before_connecting() {
        let body = "x".repeat(MAX_HTTP_REQUEST_BODY_BYTES + 1);
        assert_eq!(
            StdHttpOtlpTransport.send("http://127.0.0.1:1/v1/logs", &body),
            Err(ExportError::PayloadTooLarge)
        );
        assert_eq!(
            TlsHttpOtlpTransport::default().send("https://127.0.0.1:1/v1/logs", &body),
            Err(ExportError::PayloadTooLarge)
        );
    }

    #[test]
    fn dns_timeouts_use_a_fixed_worker_and_queue_bound() {
        let active = Arc::new(AtomicUsize::new(0));
        let maximum_active = Arc::new(AtomicUsize::new(0));
        let lookup_active = Arc::clone(&active);
        let lookup_maximum = Arc::clone(&maximum_active);
        let lookup: DnsLookup = Arc::new(move |_| {
            let current = lookup_active.fetch_add(1, Ordering::SeqCst) + 1;
            lookup_maximum.fetch_max(current, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(100));
            lookup_active.fetch_sub(1, Ordering::SeqCst);
            Vec::new()
        });
        let pool = DnsResolverPool::with_lookup(2, 1, &lookup)
            .unwrap_or_else(|_| unreachable!("fixed resolver pool"));

        for _ in 0..32 {
            assert_eq!(
                pool.resolve(
                    "collector.invalid:4318".to_owned(),
                    Duration::from_millis(1)
                ),
                Err(ExportError::Unavailable)
            );
        }

        assert!(maximum_active.load(Ordering::SeqCst) <= 2);
        let snapshot = pool.queue_snapshot();
        assert_eq!(snapshot.capacity, 2);
        assert!(snapshot.activity_total > 0);
        assert!(snapshot.process_high_water <= snapshot.capacity);
        assert!(snapshot.backpressure_total > 0);
    }

    #[test]
    fn otlp_like_exporter_sends_bounded_redacted_batches() {
        let transport = RecordingTransport {
            calls: Mutex::new(Vec::new()),
            failure: false,
        };
        let exporter = OtlpLikeExporter::new("http://collector:4318", transport)
            .unwrap_or_else(|_| unreachable!("valid endpoint"));
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::Admission {
            outcome: AdmissionOutcomeV1::Accepted,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(exporter.export(&[event.clone(), event]), Ok(()));
        let body = exporter
            .transport
            .calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first()
            .cloned()
            .unwrap_or_default();
        assert!(body.contains("\"resourceLogs\":["));
        assert!(body.contains("\"logRecords\":["));
        assert!(body.contains("\\\"duration_ms\\\":7"));
        assert!(!body.contains("password"));
        assert!(!body.contains("Bearer"));
    }

    #[test]
    fn standard_http_transport_exercises_a_local_otlp_http_collector() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .unwrap_or_else(|error| unreachable!("local collector bind: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("local collector address: {error}"));
        let collector = thread::spawn(move || {
            let (mut stream, _) = listener
                .accept()
                .unwrap_or_else(|error| unreachable!("local collector accept: {error}"));
            let mut request = Vec::new();
            stream
                .read_to_end(&mut request)
                .unwrap_or_else(|error| unreachable!("local collector request: {error}"));
            let separator = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .unwrap_or_else(|| unreachable!("HTTP header separator"));
            let body = &request[separator + 4..];
            let value: serde_json::Value = serde_json::from_slice(body)
                .unwrap_or_else(|error| unreachable!("OTLP JSON body: {error}"));
            assert!(
                value["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]["body"]["stringValue"]
                    .as_str()
                    .is_some_and(|value| value.contains(TELEMETRY_SCHEMA_V1))
            );
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap_or_else(|error| unreachable!("local collector response: {error}"));
        });
        let exporter =
            OtlpLikeExporter::new(&format!("http://{address}/v1/logs"), StdHttpOtlpTransport)
                .unwrap_or_else(|error| unreachable!("valid local collector endpoint: {error:?}"));
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::Admission {
            outcome: AdmissionOutcomeV1::Accepted,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(exporter.export(&[event]), Ok(()));
        collector
            .join()
            .unwrap_or_else(|_| unreachable!("local collector thread"));
    }

    #[test]
    fn standard_http_transport_rejects_non_success_and_oversized_responses() {
        use std::io::Read as _;
        use std::net::TcpListener;
        use std::thread;

        for (status, response_body) in [
            (503, Vec::new()),
            (200, vec![b'x'; MAX_HTTP_RESPONSE_BYTES + 1]),
        ] {
            let listener = TcpListener::bind(("127.0.0.1", 0))
                .unwrap_or_else(|error| unreachable!("local collector bind: {error}"));
            let address = listener
                .local_addr()
                .unwrap_or_else(|error| unreachable!("local collector address: {error}"));
            let collector = thread::spawn(move || {
                let (mut stream, _) = listener
                    .accept()
                    .unwrap_or_else(|error| unreachable!("local collector accept: {error}"));
                let mut request = [0_u8; 1];
                let _ = stream.read(&mut request);
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response_body.len()
                );
                let _ = std::io::Write::write_all(&mut stream, response.as_bytes());
                let _ = std::io::Write::write_all(&mut stream, &response_body);
            });
            let result = StdHttpOtlpTransport.send(&format!("http://{address}/v1/logs"), "{}");
            assert_eq!(result, Err(ExportError::Unavailable));
            collector
                .join()
                .unwrap_or_else(|_| unreachable!("local collector thread"));
        }
    }

    #[test]
    fn standard_http_transport_slow_response_is_bounded() {
        use std::io::Read as _;
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .unwrap_or_else(|error| unreachable!("local collector bind: {error}"));
        let address = listener
            .local_addr()
            .unwrap_or_else(|error| unreachable!("local collector address: {error}"));
        let collector = thread::spawn(move || {
            let (mut stream, _) = listener
                .accept()
                .unwrap_or_else(|error| unreachable!("local collector accept: {error}"));
            let mut request = [0_u8; 1];
            let _ = stream.read(&mut request);
            thread::sleep(HTTP_IO_TIMEOUT + Duration::from_millis(250));
        });
        let started = Instant::now();
        let result = StdHttpOtlpTransport.send(&format!("http://{address}/v1/logs"), "{}");
        assert_eq!(result, Err(ExportError::Unavailable));
        assert!(started.elapsed() < Duration::from_secs(3));
        collector
            .join()
            .unwrap_or_else(|_| unreachable!("local collector thread"));
    }

    #[cfg(unix)]
    struct LocalTlsCertificate {
        _directory: tempfile::TempDir,
        ca_der: Vec<u8>,
        server_der: Vec<u8>,
        server_key_der: Vec<u8>,
    }

    #[cfg(unix)]
    #[allow(clippy::too_many_lines)]
    fn local_tls_certificate() -> anyhow::Result<LocalTlsCertificate> {
        use std::process::Command;

        let directory = tempfile::tempdir()?;
        let path = |name: &str| directory.path().join(name);
        let ca_key = path("ca.key");
        let ca_pem = path("ca.pem");
        let server_key = path("server.key");
        let server_csr = path("server.csr");
        let server_pem = path("server.pem");
        let server_ext = path("server.ext");
        let ca_der = path("ca.der");
        let server_der = path("server.der");
        let server_key_der = path("server.key.der");

        let run = |args: &[&str]| -> anyhow::Result<()> {
            let output = Command::new("openssl").args(args).output()?;
            if !output.status.success() {
                anyhow::bail!("openssl test certificate command failed");
            }
            Ok(())
        };
        let ca_key_string = ca_key.to_string_lossy().into_owned();
        let ca_pem_string = ca_pem.to_string_lossy().into_owned();
        run(&[
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            &ca_key_string,
            "-out",
            &ca_pem_string,
            "-subj",
            "/CN=WorldStream test CA",
            "-days",
            "1",
        ])?;

        let server_key_string = server_key.to_string_lossy().into_owned();
        let server_csr_string = server_csr.to_string_lossy().into_owned();
        run(&[
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-keyout",
            &server_key_string,
            "-out",
            &server_csr_string,
            "-subj",
            "/CN=localhost",
        ])?;
        std::fs::write(
            &server_ext,
            "basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:localhost\n",
        )?;
        let server_pem_string = server_pem.to_string_lossy().into_owned();
        let server_ext_string = server_ext.to_string_lossy().into_owned();
        run(&[
            "x509",
            "-req",
            "-in",
            &server_csr_string,
            "-CA",
            &ca_pem_string,
            "-CAkey",
            &ca_key_string,
            "-CAcreateserial",
            "-out",
            &server_pem_string,
            "-days",
            "1",
            "-sha256",
            "-extfile",
            &server_ext_string,
        ])?;

        let ca_der_string = ca_der.to_string_lossy().into_owned();
        let server_der_string = server_der.to_string_lossy().into_owned();
        let server_key_der_string = server_key_der.to_string_lossy().into_owned();
        run(&[
            "x509",
            "-in",
            &ca_pem_string,
            "-outform",
            "DER",
            "-out",
            &ca_der_string,
        ])?;
        run(&[
            "x509",
            "-in",
            &server_pem_string,
            "-outform",
            "DER",
            "-out",
            &server_der_string,
        ])?;
        run(&[
            "pkcs8",
            "-topk8",
            "-nocrypt",
            "-in",
            &server_key_string,
            "-outform",
            "DER",
            "-out",
            &server_key_der_string,
        ])?;

        Ok(LocalTlsCertificate {
            _directory: directory,
            ca_der: std::fs::read(ca_der)?,
            server_der: std::fs::read(server_der)?,
            server_key_der: std::fs::read(server_key_der)?,
        })
    }

    #[cfg(unix)]
    fn spawn_tls_collector(
        certificate: &LocalTlsCertificate,
        status: u16,
        response_body: &[u8],
    ) -> anyhow::Result<(String, std::thread::JoinHandle<anyhow::Result<Vec<u8>>>)> {
        use rustls::{ServerConfig, ServerConnection};
        use std::io::{Error, ErrorKind, Read as _};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        let address = listener.local_addr()?;
        let server_config = ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    certificate.server_der.clone(),
                )],
                rustls::pki_types::PrivateKeyDer::Pkcs8(
                    rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.server_key_der.clone()),
                ),
            )?;
        let response_body = response_body.to_vec();
        let collector = thread::spawn(move || {
            let (stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(HTTP_IO_TIMEOUT))?;
            stream.set_write_timeout(Some(HTTP_IO_TIMEOUT))?;
            let mut stream =
                StreamOwned::new(ServerConnection::new(Arc::new(server_config))?, stream);
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            let body_start = loop {
                let count = stream.read(&mut chunk)?;
                if count == 0 {
                    return Err(anyhow::anyhow!("TLS collector received EOF before headers"));
                }
                request.extend_from_slice(&chunk[..count]);
                if request.len() > MAX_HTTP_REQUEST_BODY_BYTES + 4096 {
                    return Err(anyhow::anyhow!("TLS collector request exceeded its bound"));
                }
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = std::str::from_utf8(&request[..body_start])?;
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.strip_prefix("Content-Length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .ok_or_else(|| anyhow::anyhow!("TLS collector request omitted Content-Length"))?;
            if content_length > MAX_HTTP_REQUEST_BODY_BYTES {
                return Err(anyhow::anyhow!("TLS collector body exceeded its bound"));
            }
            while request.len() < body_start + content_length {
                let count = stream.read(&mut chunk)?;
                if count == 0 {
                    return Err(anyhow::anyhow!("TLS collector received a truncated body"));
                }
                request.extend_from_slice(&chunk[..count]);
                if request.len() > body_start + content_length {
                    return Err(Error::new(
                        ErrorKind::InvalidData,
                        "TLS collector received bytes beyond Content-Length",
                    )
                    .into());
                }
            }
            request.truncate(body_start + content_length);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(response.as_bytes())?;
            stream.write_all(&response_body)?;
            stream.flush()?;
            stream.conn.send_close_notify();
            stream.flush()?;
            stream.get_mut().shutdown(Shutdown::Write)?;
            Ok(request)
        });
        Ok((
            format!("https://localhost:{}/v1/logs", address.port()),
            collector,
        ))
    }

    #[cfg(unix)]
    #[test]
    fn tls_transport_uses_trusted_ca_hostname_verification_and_otlp_http() -> anyhow::Result<()> {
        let certificate = local_tls_certificate()?;
        let mut roots = RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(
            certificate.ca_der.clone(),
        ))?;
        let transport = TlsHttpOtlpTransport::with_root_store(roots);
        let (endpoint, collector) = spawn_tls_collector(&certificate, 204, &[])?;
        let exporter = OtlpLikeExporter::new(&endpoint, transport)
            .unwrap_or_else(|error| unreachable!("valid local TLS endpoint: {error:?}"));
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::Admission {
            outcome: AdmissionOutcomeV1::Accepted,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        let export_result = exporter.export(&[event]);
        let collector_result = collector
            .join()
            .unwrap_or_else(|_| unreachable!("local TLS collector thread"));
        assert_eq!(export_result, Ok(()));
        let request = collector_result?;
        let header_end = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| anyhow::anyhow!("TLS collector did not receive HTTP headers"))?;
        let headers = std::str::from_utf8(&request[..header_end])?;
        assert!(headers.starts_with("POST /v1/logs HTTP/1.1\r\n"));
        assert!(headers.contains("Content-Type: application/json\r\n"));
        let body = &request[header_end + 4..];
        assert!(body.len() <= MAX_HTTP_REQUEST_BODY_BYTES);
        let value: serde_json::Value = serde_json::from_slice(body)?;
        assert!(
            value["resourceLogs"][0]["scopeLogs"][0]["logRecords"]
                .as_array()
                .is_some_and(|records| !records.is_empty())
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn tls_transport_rejects_untrusted_cert_and_hostname_mismatch() -> anyhow::Result<()> {
        let certificate = local_tls_certificate()?;
        let (endpoint, collector) = spawn_tls_collector(&certificate, 200, &[])?;
        let result = TlsHttpOtlpTransport::default().send(&endpoint, "{}");
        assert_eq!(result, Err(ExportError::Unavailable));
        let _ = collector.join();

        let mut roots = RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(
            certificate.ca_der.clone(),
        ))?;
        let transport = TlsHttpOtlpTransport::with_root_store(roots);
        let (endpoint, collector) = spawn_tls_collector(&certificate, 200, &[])?;
        let mismatch = endpoint.replacen("localhost", "127.0.0.1", 1);
        assert_eq!(
            transport.send(&mismatch, "{}"),
            Err(ExportError::Unavailable)
        );
        let _ = collector.join();
        Ok(())
    }

    #[test]
    fn standard_http_transport_exercises_configured_external_collector() {
        let Ok(endpoint) = std::env::var("WORLDSTREAM_TELEMETRY_COLLECTOR_ENDPOINT") else {
            eprintln!(
                "external OTLP collector not configured; no collector-delivery claim is made"
            );
            return;
        };
        let exporter = OtlpLikeExporter::new(&endpoint, StdHttpOtlpTransport)
            .unwrap_or_else(|error| unreachable!("configured collector endpoint: {error:?}"));
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::StorageDiagnostic {
            kind: StorageDiagnosticKindV1::Checkpoint,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(exporter.export(&[event]), Ok(()));
    }

    #[test]
    fn exporter_failures_have_explicit_retry_classes() {
        assert_eq!(
            ExportError::Unavailable.retry_class(),
            ExportRetryClassV1::RetryableOutage
        );
        assert_eq!(
            ExportError::MalformedEndpoint.retry_class(),
            ExportRetryClassV1::PermanentConfiguration
        );
        assert_eq!(
            ExportError::InvalidEvent.retry_class(),
            ExportRetryClassV1::PermanentPayload
        );
    }

    struct NoopCollector;

    impl TelemetryExporter for NoopCollector {
        fn export(&self, _: &[TelemetryEventV1]) -> Result<(), ExportError> {
            Ok(())
        }
    }

    struct FailingCollector;

    impl TelemetryExporter for FailingCollector {
        fn export(&self, _: &[TelemetryEventV1]) -> Result<(), ExportError> {
            Err(ExportError::Unavailable)
        }
    }

    struct PanickingCollector;

    impl TelemetryExporter for PanickingCollector {
        fn export(&self, _: &[TelemetryEventV1]) -> Result<(), ExportError> {
            std::panic::resume_unwind(Box::new("collector failure"));
        }
    }

    #[test]
    fn submit_event_cannot_bypass_bounded_redaction() {
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 1,
                batch_size: 1,
                ..TelemetryConfig::default()
            },
            Arc::new(NoopCollector),
        )
        .unwrap_or_else(|_| unreachable!("valid runtime"));
        let handle = runtime.handle();
        let mut event = TelemetryEventV1::redact(draft(EventDetailsV1::Admission {
            outcome: AdmissionOutcomeV1::Accepted,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        let mut wire = serde_json::to_value(&event)
            .unwrap_or_else(|error| unreachable!("valid telemetry JSON: {error}"));
        wire.as_object_mut()
            .unwrap_or_else(|| unreachable!("telemetry object"))
            .insert("private_payload".to_owned(), serde_json::json!("secret"));
        assert!(serde_json::from_value::<TelemetryEventV1>(wire).is_err());
        event.attributes.insert("room_id".to_owned(), 7);
        assert_eq!(handle.submit_event(event), QueueSubmitResult::Dropped);
        assert_eq!(runtime.metrics().snapshot().overflow_warnings, 0);
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            FlushOutcome::Flushed
        );
    }

    #[test]
    fn exporter_outage_is_counted_and_does_not_block_shutdown() {
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 1,
                batch_size: 1,
                ..TelemetryConfig::default()
            },
            Arc::new(FailingCollector),
        )
        .unwrap_or_else(|_| unreachable!("valid runtime"));
        let handle = runtime.handle();
        let metrics = runtime.metrics();
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::Recovery {
            phase: RecoveryPhaseV1::Started,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(handle.submit_event(event), QueueSubmitResult::Enqueued);
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            FlushOutcome::Flushed
        );
        assert_eq!(metrics.snapshot().exporter_failures, 1);
        assert_eq!(metrics.snapshot().exporter_retryable_failures, 1);
        assert_eq!(
            metrics.health_snapshot(1, false).exporter,
            ExporterHealthV1::Unavailable
        );
    }

    #[test]
    fn health_snapshot_exposes_bounded_queue_and_stopped_state() {
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 2,
                batch_size: 2,
                ..TelemetryConfig::default()
            },
            Arc::new(NoopCollector),
        )
        .unwrap_or_else(|_| unreachable!("valid runtime"));
        let health = runtime.health_snapshot();
        assert_eq!(health.queue_capacity, 2);
        assert_eq!(health.queue_depth, 0);
        assert_eq!(health.exporter, ExporterHealthV1::Healthy);
        let metrics = runtime.metrics();
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            FlushOutcome::Flushed
        );
        assert_eq!(
            metrics.health_snapshot(2, true).exporter,
            ExporterHealthV1::Stopped
        );
    }

    #[test]
    fn panicking_exporter_is_isolated_from_runtime_shutdown() {
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 1,
                batch_size: 1,
                ..TelemetryConfig::default()
            },
            Arc::new(PanickingCollector),
        )
        .unwrap_or_else(|_| unreachable!("valid runtime"));
        let handle = runtime.handle();
        let metrics = runtime.metrics();
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::Recovery {
            phase: RecoveryPhaseV1::Started,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        assert_eq!(handle.submit_event(event), QueueSubmitResult::Enqueued);
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            FlushOutcome::Flushed
        );
        assert_eq!(metrics.snapshot().exporter_failures, 1);
    }

    #[test]
    fn sqlite_bridge_preserves_adapter_and_exact_bounded_storage_facts() {
        #[derive(Clone, Default)]
        struct Capture(Arc<Mutex<Vec<TelemetryEventV1>>>);

        impl TelemetryExporter for Capture {
            fn export(&self, batch: &[TelemetryEventV1]) -> Result<(), ExportError> {
                self.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend_from_slice(batch);
                Ok(())
            }
        }

        let exporter = Capture::default();
        let captured = Arc::clone(&exporter.0);
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 8,
                batch_size: 8,
                ..TelemetryConfig::default()
            },
            Arc::new(exporter),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let bridge = SqliteTelemetryBridge::new(runtime.handle());
        bridge.emit(SqliteTelemetryEventV1::Migration {
            phase: SqliteMigrationPhaseV1::Started,
            schema_version: 8,
        });
        bridge.emit(SqliteTelemetryEventV1::Recovery {
            phase: SqliteRecoveryPhaseV1::Failed,
        });
        bridge.emit(SqliteTelemetryEventV1::Integrity {
            status: SqliteIntegrityStatusV1::Quarantined,
        });
        bridge.emit(SqliteTelemetryEventV1::StorageDiagnostic {
            kind: SqliteStorageDiagnosticKindV1::Query,
        });
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            FlushOutcome::Flushed
        );
        let events = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|event| {
            event.adapter == Some(AdapterIdentityV1::Sqlite)
                && event.correlation == CorrelationV1::none()
                && event.json_line().is_ok()
        }));
        assert_eq!(events[0].event, EventKindV1::Migration);
        assert_eq!(events[0].reason, ReasonCodeV1::Accepted);
        assert_eq!(events[0].attributes.get("schema_version"), Some(&8));
        assert_eq!(events[1].reason, ReasonCodeV1::RecoveryRequired);
        assert_eq!(events[2].reason, ReasonCodeV1::IntegrityFailure);
        assert_eq!(events[3].reason, ReasonCodeV1::StorageUnavailable);
        let encoded = events
            .iter()
            .map(|event| event.json_line().unwrap_or_default())
            .collect::<String>();
        assert!(!encoded.contains("room_id"));
        assert!(!encoded.contains("canonical"));
        assert!(!encoded.contains("endpoint"));
    }

    #[test]
    fn postgres_bridge_preserves_adapter_and_exact_bounded_storage_facts() {
        #[derive(Clone, Default)]
        struct Capture(Arc<Mutex<Vec<TelemetryEventV1>>>);

        impl TelemetryExporter for Capture {
            fn export(&self, batch: &[TelemetryEventV1]) -> Result<(), ExportError> {
                self.0
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .extend_from_slice(batch);
                Ok(())
            }
        }

        let exporter = Capture::default();
        let captured = Arc::clone(&exporter.0);
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 8,
                batch_size: 8,
                ..TelemetryConfig::default()
            },
            Arc::new(exporter),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let bridge = PostgresTelemetryBridge::new(runtime.handle());
        bridge.emit(PostgresTelemetryEventV1::Migration {
            phase: PostgresMigrationPhaseV1::Started,
            schema_version: 8,
        });
        bridge.emit(PostgresTelemetryEventV1::Recovery {
            phase: PostgresRecoveryPhaseV1::Failed,
        });
        bridge.emit(PostgresTelemetryEventV1::Integrity {
            status: PostgresIntegrityStatusV1::Quarantined,
        });
        bridge.emit(PostgresTelemetryEventV1::StorageDiagnostic {
            kind: PostgresStorageDiagnosticKindV1::Lock,
        });
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            FlushOutcome::Flushed
        );
        let events = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(events.len(), 4);
        assert!(events.iter().all(|event| {
            event.adapter == Some(AdapterIdentityV1::Postgres)
                && event.correlation == CorrelationV1::none()
                && event.json_line().is_ok()
        }));
        assert_eq!(events[0].event, EventKindV1::Migration);
        assert_eq!(events[0].reason, ReasonCodeV1::Accepted);
        assert_eq!(events[0].attributes.get("schema_version"), Some(&8));
        assert_eq!(events[1].reason, ReasonCodeV1::RecoveryRequired);
        assert_eq!(events[2].reason, ReasonCodeV1::IntegrityFailure);
        assert_eq!(events[3].reason, ReasonCodeV1::Busy);
        let encoded = events
            .iter()
            .map(|event| event.json_line().unwrap_or_default())
            .collect::<String>();
        assert!(!encoded.contains("room_id"));
        assert!(!encoded.contains("canonical"));
        assert!(!encoded.contains("endpoint"));
        assert!(!encoded.contains("dsn"));
        assert!(!encoded.contains("sql"));
    }

    #[test]
    fn postgres_bridge_is_nonblocking_when_queue_is_saturated() {
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 1,
                batch_size: 1,
                ..TelemetryConfig::default()
            },
            Arc::new(SlowCollector),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let bridge = PostgresTelemetryBridge::new(runtime.handle());
        let started = Instant::now();
        for _ in 0..10_000 {
            bridge.emit(PostgresTelemetryEventV1::StorageDiagnostic {
                kind: PostgresStorageDiagnosticKindV1::Query,
            });
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        let metrics = runtime.metrics();
        let _ = runtime.shutdown(Duration::from_secs(1));
        assert!(metrics.snapshot().dropped > 0);
        let queue = metrics.queue_snapshot(1);
        assert_eq!(queue.capacity, 1);
        assert_eq!(queue.unit_high_water, 1);
        assert!(queue.process_current <= queue.capacity);
        assert!(queue.process_high_water <= queue.capacity);
        assert!(queue.activity_total > 0);
        assert!(queue.completion_total > 0);
        assert!(queue.backpressure_total > 0);
    }

    #[test]
    fn sqlite_storage_stays_truthful_when_exporter_fails() {
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 8,
                batch_size: 1,
                ..TelemetryConfig::default()
            },
            Arc::new(FailingCollector),
        )
        .unwrap_or_else(|error| unreachable!("telemetry runtime: {error:?}"));
        let bridge = SqliteTelemetryBridge::new(runtime.handle());
        let metrics = runtime.metrics();
        let directory =
            tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp directory: {error}"));
        let path = directory.path().join("telemetry-failure.sqlite3");
        let store =
            worldstream_sqlite::SqliteRoomStore::open_with_telemetry(&path, Some(Arc::new(bridge)))
                .unwrap_or_else(|error| unreachable!("SQLite open: {error}"));
        let initialized = store
            .initialize_canonical_metadata("deployment/telemetry", 3)
            .unwrap_or_else(|error| unreachable!("metadata: {error}"));
        assert_eq!(
            initialized,
            worldstream_sqlite::SqliteCanonicalMetadataInitializationV1::Initialized
        );
        assert_eq!(
            store.engine_identity().0,
            worldstream_sqlite::SQLITE_VERSION
        );
        drop(store);
        let reopened = worldstream_sqlite::SqliteRoomStore::open(&path)
            .unwrap_or_else(|error| unreachable!("SQLite reopen: {error}"));
        assert_eq!(
            reopened
                .initialize_canonical_metadata("deployment/telemetry", 3)
                .unwrap_or_else(|error| unreachable!("metadata replay: {error}")),
            worldstream_sqlite::SqliteCanonicalMetadataInitializationV1::AlreadyInitialized
        );
        drop(reopened);
        assert_eq!(
            runtime.shutdown(Duration::from_secs(1)),
            FlushOutcome::Flushed
        );
        assert!(metrics.snapshot().exporter_failures > 0);
    }

    struct SlowCollector;

    impl TelemetryExporter for SlowCollector {
        fn export(&self, _: &[TelemetryEventV1]) -> Result<(), ExportError> {
            thread::sleep(Duration::from_millis(40));
            Ok(())
        }
    }

    #[test]
    fn queue_is_nonblocking_bounded_and_shutdown_is_time_limited() {
        let runtime = TelemetryRuntime::new(
            TelemetryConfig {
                queue_capacity: 1,
                batch_size: 1,
                slow_export_after: Duration::from_millis(1),
            },
            Arc::new(SlowCollector),
        )
        .unwrap_or_else(|_| unreachable!("valid runtime"));
        let handle = runtime.handle();
        let metrics = runtime.metrics();
        let event = TelemetryEventV1::redact(draft(EventDetailsV1::Timer {
            phase: TimerPhaseV1::Scanned,
        }))
        .unwrap_or_else(|| unreachable!("matching typed details"));
        for _ in 0..32 {
            let _ = handle.submit_event(event.clone());
        }
        assert!(metrics.snapshot().dropped > 0);
        assert!(metrics.snapshot().overflow_warnings > 0);
        let started = Instant::now();
        assert_eq!(
            runtime.shutdown(Duration::from_millis(5)),
            FlushOutcome::TimedOut
        );
        assert!(started.elapsed() < Duration::from_millis(100));
        thread::sleep(Duration::from_millis(60));
        assert!(metrics.snapshot().exporter_slow > 0);
    }
}

//! Bounded, identity-safe gateway admission control.
//!
//! Callers present the dimensions that are already known at their transport
//! seam. The limiter hashes every identifier with a process-local key, checks
//! every presented bucket atomically, and retains neither bearer material nor
//! raw principal, session, Capability, Membership, or Room identifiers.

use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const MAX_KEY_MATERIAL_BYTES: usize = 512;
const CAPABILITY_HASH_BYTES: usize = 32;
const MAX_KEYS_PER_ADMISSION: usize = 64;
const MAX_TARGETS_PER_PRINCIPAL: usize = 256;
const MAX_TARGET_BINDINGS: usize = 65_536;
const MAX_PENDING_WEBSOCKETS: usize = 256;
const MAX_ACTIVE_WEBSOCKETS: usize = 4_096;
const MAX_ACTIVE_WEBSOCKETS_PER_PRINCIPAL: usize = 64;
const EXTRA_METRIC_LABELS: [&str; 7] = [
    "capacity",
    "target_capacity",
    "pending_connection",
    "active_connection",
    "principal_connection",
    "invalid_identity",
    "clock",
];
const METRIC_SCOPE_COUNT: usize = AdmissionDimension::ALL.len() + EXTRA_METRIC_LABELS.len();

/// One transport peer. Missing connection metadata is deliberately collapsed
/// into one conservative bucket instead of bypassing the IP limit.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum PeerIdentity {
    Observed(IpAddr),
    Unattributed,
}

impl PeerIdentity {
    pub(crate) fn from_ip(ip: Option<IpAddr>) -> Self {
        match ip {
            Some(IpAddr::V6(ipv6)) => ipv6
                .to_ipv4_mapped()
                .map_or(Self::Observed(IpAddr::V6(ipv6)), |ipv4| {
                    Self::Observed(IpAddr::V4(ipv4))
                }),
            Some(ip) => Self::Observed(ip),
            None => Self::Unattributed,
        }
    }
}

/// An optional Room and Membership target for one admitted operation.
#[derive(Clone, Copy)]
pub(crate) struct AdmissionTarget<'a> {
    pub(crate) room_id: &'a str,
    pub(crate) member_id: Option<&'a str>,
}

/// All independently bounded dimensions known for one admission decision.
#[derive(Clone, Copy, Default)]
pub(crate) struct GatewayAdmission<'a> {
    pub(crate) peer: Option<PeerIdentity>,
    pub(crate) principal_id: Option<&'a str>,
    pub(crate) capability_material: Option<&'a [u8]>,
    pub(crate) session_id: Option<&'a str>,
    pub(crate) activation_operation: Option<&'a str>,
    pub(crate) operator_endpoint: Option<&'a str>,
    pub(crate) targets: &'a [AdmissionTarget<'a>],
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AdmissionDimension {
    Ip,
    Principal,
    Capability,
    Session,
    Membership,
    Room,
    Activation,
    Operator,
}

impl AdmissionDimension {
    const ALL: [Self; 8] = [
        Self::Ip,
        Self::Principal,
        Self::Capability,
        Self::Session,
        Self::Membership,
        Self::Room,
        Self::Activation,
        Self::Operator,
    ];

    const fn index(self) -> usize {
        match self {
            Self::Ip => 0,
            Self::Principal => 1,
            Self::Capability => 2,
            Self::Session => 3,
            Self::Membership => 4,
            Self::Room => 5,
            Self::Activation => 6,
            Self::Operator => 7,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Ip => "ip",
            Self::Principal => "principal",
            Self::Capability => "capability",
            Self::Session => "session",
            Self::Membership => "membership",
            Self::Room => "room",
            Self::Activation => "activation",
            Self::Operator => "operator",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RateLimitRejection {
    Exhausted(AdmissionDimension),
    Capacity,
    TargetCapacity,
    PendingConnectionCapacity,
    ActiveConnectionCapacity,
    PrincipalConnectionCapacity,
    InvalidIdentity,
    Clock,
}

#[derive(Clone, Copy)]
struct Quota {
    burst: u32,
    refill_every: Duration,
    max_keys: usize,
}

#[derive(Clone)]
struct LimiterConfig {
    quotas: [Quota; AdmissionDimension::ALL.len()],
    idle_retention: Duration,
    max_targets_per_principal: usize,
    max_target_bindings: usize,
    max_pending_websockets: usize,
    max_active_websockets: usize,
    max_active_websockets_per_principal: usize,
}

impl LimiterConfig {
    fn production() -> Self {
        Self {
            quotas: [
                Quota {
                    burst: 1_024,
                    refill_every: Duration::from_millis(2),
                    max_keys: 4_096,
                },
                Quota {
                    burst: 512,
                    refill_every: Duration::from_millis(4),
                    max_keys: 4_096,
                },
                Quota {
                    burst: 256,
                    refill_every: Duration::from_millis(8),
                    max_keys: 8_192,
                },
                Quota {
                    burst: 256,
                    refill_every: Duration::from_millis(8),
                    max_keys: 8_192,
                },
                Quota {
                    burst: 128,
                    refill_every: Duration::from_millis(16),
                    max_keys: 8_192,
                },
                Quota {
                    burst: 512,
                    refill_every: Duration::from_millis(4),
                    max_keys: 8_192,
                },
                Quota {
                    burst: 64,
                    refill_every: Duration::from_millis(32),
                    max_keys: 8_192,
                },
                Quota {
                    burst: 128,
                    refill_every: Duration::from_millis(16),
                    max_keys: 8_192,
                },
            ],
            idle_retention: Duration::from_mins(10),
            max_targets_per_principal: MAX_TARGETS_PER_PRINCIPAL,
            max_target_bindings: MAX_TARGET_BINDINGS,
            max_pending_websockets: MAX_PENDING_WEBSOCKETS,
            max_active_websockets: MAX_ACTIVE_WEBSOCKETS,
            max_active_websockets_per_principal: MAX_ACTIVE_WEBSOCKETS_PER_PRINCIPAL,
        }
    }

    #[cfg(test)]
    fn uniform(burst: u32, refill_every: Duration, max_keys: usize) -> Self {
        Self {
            quotas: [Quota {
                burst,
                refill_every,
                max_keys,
            }; AdmissionDimension::ALL.len()],
            idle_retention: Duration::from_mins(1),
            max_targets_per_principal: max_keys.min(MAX_TARGETS_PER_PRINCIPAL),
            max_target_bindings: max_keys.saturating_mul(4).max(1),
            max_pending_websockets: max_keys.max(1),
            max_active_websockets: max_keys.max(1),
            max_active_websockets_per_principal: max_keys.max(1),
        }
    }
}

trait Clock: Send + Sync + 'static {
    fn now(&self) -> Duration;
}

struct MonotonicClock {
    epoch: Instant,
}

impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct AdmissionKey {
    dimension: AdmissionDimension,
    fingerprint: [u8; 32],
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct PrincipalTargetKey {
    principal: [u8; 32],
    target: AdmissionKey,
}

struct PreparedAdmission {
    keys: Vec<AdmissionKey>,
    target_bindings: Vec<PrincipalTargetKey>,
}

impl std::fmt::Debug for AdmissionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AdmissionKey")
            .field("dimension", &self.dimension)
            .field("fingerprint", &"[REDACTED]")
            .finish()
    }
}

#[derive(Clone, Copy)]
struct Bucket {
    available: u32,
    last_refill: Duration,
    last_seen: Duration,
}

struct LimiterState {
    buckets: HashMap<AdmissionKey, Bucket>,
    counts: [usize; AdmissionDimension::ALL.len()],
    target_bindings: HashMap<PrincipalTargetKey, Duration>,
    target_counts: HashMap<[u8; 32], usize>,
    pending_websockets: usize,
    active_websockets: usize,
    active_websockets_by_principal: HashMap<[u8; 32], usize>,
    last_now: Duration,
    next_sweep: Duration,
}

impl LimiterState {
    fn new() -> Self {
        Self {
            buckets: HashMap::new(),
            counts: [0; AdmissionDimension::ALL.len()],
            target_bindings: HashMap::new(),
            target_counts: HashMap::new(),
            pending_websockets: 0,
            active_websockets: 0,
            active_websockets_by_principal: HashMap::new(),
            last_now: Duration::ZERO,
            next_sweep: Duration::ZERO,
        }
    }

    fn expire_idle(&mut self, now: Duration, retention: Duration) {
        if now < self.next_sweep {
            return;
        }
        let mut expired = [0_usize; AdmissionDimension::ALL.len()];
        self.buckets.retain(|key, bucket| {
            let retain = now.saturating_sub(bucket.last_seen) < retention;
            if !retain {
                expired[key.dimension.index()] += 1;
            }
            retain
        });
        for (count, removed) in self.counts.iter_mut().zip(expired) {
            *count = count.saturating_sub(removed);
        }
        let expired_bindings = self
            .target_bindings
            .iter()
            .filter_map(|(key, last_seen)| {
                (now.saturating_sub(*last_seen) >= retention).then_some(*key)
            })
            .collect::<Vec<_>>();
        for key in expired_bindings {
            self.target_bindings.remove(&key);
            if let std::collections::hash_map::Entry::Occupied(mut count) =
                self.target_counts.entry(key.principal)
            {
                *count.get_mut() = count.get().saturating_sub(1);
                if *count.get() == 0 {
                    count.remove();
                }
            }
        }
        let sweep_interval = retention.min(Duration::from_secs(30));
        self.next_sweep = now.checked_add(sweep_interval).unwrap_or(now);
    }
}

struct LimiterMetrics {
    rejections: [AtomicU64; METRIC_SCOPE_COUNT],
}

impl LimiterMetrics {
    fn new() -> Self {
        Self {
            rejections: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    fn record(&self, rejection: RateLimitRejection) {
        let index = match rejection {
            RateLimitRejection::Exhausted(dimension) => dimension.index(),
            RateLimitRejection::Capacity => AdmissionDimension::ALL.len(),
            RateLimitRejection::TargetCapacity => AdmissionDimension::ALL.len() + 1,
            RateLimitRejection::PendingConnectionCapacity => AdmissionDimension::ALL.len() + 2,
            RateLimitRejection::ActiveConnectionCapacity => AdmissionDimension::ALL.len() + 3,
            RateLimitRejection::PrincipalConnectionCapacity => AdmissionDimension::ALL.len() + 4,
            RateLimitRejection::InvalidIdentity => AdmissionDimension::ALL.len() + 5,
            RateLimitRejection::Clock => AdmissionDimension::ALL.len() + 6,
        };
        self.rejections[index].fetch_add(1, Ordering::Relaxed);
    }
}

struct LimiterInner {
    clock: Arc<dyn Clock>,
    config: LimiterConfig,
    fingerprint_key: [u8; 32],
    state: Mutex<LimiterState>,
    metrics: LimiterMetrics,
}

/// Shared process limiter used by both HTTP and WebSocket admission paths.
#[derive(Clone)]
pub(crate) struct GatewayRateLimiter {
    inner: Arc<LimiterInner>,
}

/// One bounded pre-authentication `WebSocket` reservation.
///
/// Dropping this value releases the pending slot. A successful activation
/// consumes it and returns an active permit instead.
pub(crate) struct PendingWebSocketPermit {
    inner: Arc<LimiterInner>,
    held: bool,
}

/// One bounded authenticated `WebSocket` reservation.
///
/// The stored identity is a process-keyed Principal fingerprint. Dropping the
/// value releases both the global and per-Principal active slots.
pub(crate) struct ActiveWebSocketPermit {
    inner: Arc<LimiterInner>,
    principal: [u8; 32],
    held: bool,
}

impl GatewayRateLimiter {
    pub(crate) fn new() -> Result<Self, getrandom::Error> {
        let mut fingerprint_key = [0_u8; 32];
        getrandom::fill(&mut fingerprint_key)?;
        Ok(Self::with_parts(
            LimiterConfig::production(),
            Arc::new(MonotonicClock {
                epoch: Instant::now(),
            }),
            fingerprint_key,
        ))
    }

    fn with_parts(config: LimiterConfig, clock: Arc<dyn Clock>, fingerprint_key: [u8; 32]) -> Self {
        Self {
            inner: Arc::new(LimiterInner {
                clock,
                config,
                fingerprint_key,
                state: Mutex::new(LimiterState::new()),
                metrics: LimiterMetrics::new(),
            }),
        }
    }

    /// Atomically consumes one token from every presented dimension.
    ///
    /// A malformed key, clock regression, poisoned state, or full key store
    /// rejects the operation. No partial token consumption occurs when a
    /// quota or capacity check rejects the admission.
    pub(crate) fn admit(&self, admission: GatewayAdmission<'_>) -> Result<(), RateLimitRejection> {
        let prepared = match self.prepare(admission) {
            Ok(prepared) => prepared,
            Err(rejection) => {
                self.inner.metrics.record(rejection);
                return Err(rejection);
            }
        };
        if prepared.keys.is_empty() {
            self.inner
                .metrics
                .record(RateLimitRejection::InvalidIdentity);
            return Err(RateLimitRejection::InvalidIdentity);
        }
        let now = self.inner.clock.now();
        let Ok(mut state) = self.inner.state.lock() else {
            self.inner.metrics.record(RateLimitRejection::Capacity);
            return Err(RateLimitRejection::Capacity);
        };
        let result = self.admit_prepared(&mut state, prepared, now);
        if let Err(rejection) = result {
            self.inner.metrics.record(rejection);
        }
        result
    }

    fn admit_prepared(
        &self,
        state: &mut LimiterState,
        prepared: PreparedAdmission,
        now: Duration,
    ) -> Result<(), RateLimitRejection> {
        if now < state.last_now {
            return Err(RateLimitRejection::Clock);
        }
        state.last_now = now;
        state.expire_idle(now, self.inner.config.idle_retention);

        let target_additions = prepared
            .target_bindings
            .iter()
            .filter(|binding| !state.target_bindings.contains_key(binding))
            .count();
        if let Some(first) = prepared.target_bindings.first()
            && (state
                .target_counts
                .get(&first.principal)
                .copied()
                .unwrap_or(0)
                .saturating_add(target_additions)
                > self.inner.config.max_targets_per_principal
                || state.target_bindings.len().saturating_add(target_additions)
                    > self.inner.config.max_target_bindings)
        {
            return Err(RateLimitRejection::TargetCapacity);
        }

        let mut additions = [0_usize; AdmissionDimension::ALL.len()];
        for key in &prepared.keys {
            if !state.buckets.contains_key(key) {
                additions[key.dimension.index()] += 1;
            }
        }
        if AdmissionDimension::ALL.into_iter().any(|dimension| {
            let index = dimension.index();
            state.counts[index].saturating_add(additions[index])
                > self.inner.config.quotas[index].max_keys
        }) {
            return Err(RateLimitRejection::Capacity);
        }

        for key in &prepared.keys {
            let Some(bucket) = state.buckets.get(key) else {
                continue;
            };
            let quota = self.inner.config.quotas[key.dimension.index()];
            if refilled(*bucket, quota, now).available == 0 {
                if let Some(bucket) = state.buckets.get_mut(key) {
                    bucket.last_seen = now;
                }
                for binding in &prepared.target_bindings {
                    if let Some(last_seen) = state.target_bindings.get_mut(binding) {
                        *last_seen = now;
                    }
                }
                let rejection = RateLimitRejection::Exhausted(key.dimension);
                return Err(rejection);
            }
        }

        for key in prepared.keys {
            let quota = self.inner.config.quotas[key.dimension.index()];
            match state.buckets.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let mut bucket = refilled(*entry.get(), quota, now);
                    bucket.available -= 1;
                    bucket.last_seen = now;
                    entry.insert(bucket);
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(Bucket {
                        available: quota.burst - 1,
                        last_refill: now,
                        last_seen: now,
                    });
                    state.counts[key.dimension.index()] += 1;
                }
            }
        }
        for binding in prepared.target_bindings {
            match state.target_bindings.entry(binding) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    entry.insert(now);
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(now);
                    *state.target_counts.entry(binding.principal).or_insert(0) += 1;
                }
            }
        }
        Ok(())
    }

    fn prepare(
        &self,
        admission: GatewayAdmission<'_>,
    ) -> Result<PreparedAdmission, RateLimitRejection> {
        if admission.principal_id.is_none()
            && (!admission.targets.is_empty()
                || admission.capability_material.is_some()
                || admission.activation_operation.is_some()
                || admission.operator_endpoint.is_some())
        {
            return Err(RateLimitRejection::InvalidIdentity);
        }
        let mut keys = Vec::new();
        if let Some(peer) = admission.peer {
            match peer {
                PeerIdentity::Observed(ip) => {
                    let material = ip.to_string();
                    self.push_key(&mut keys, AdmissionDimension::Ip, &[material.as_bytes()])?;
                }
                PeerIdentity::Unattributed => {
                    self.push_key(&mut keys, AdmissionDimension::Ip, &[b"unattributed-peer"])?;
                }
            }
        }
        let principal = admission
            .principal_id
            .map(|principal_id| {
                self.push_key(
                    &mut keys,
                    AdmissionDimension::Principal,
                    &[principal_id.as_bytes()],
                )
            })
            .transpose()?;
        let capability_material = admission
            .capability_material
            .map(Self::encode_capability_material)
            .transpose()?;
        capability_material
            .as_ref()
            .map(|capability_material| {
                self.push_key(
                    &mut keys,
                    AdmissionDimension::Capability,
                    &[capability_material.as_slice()],
                )
            })
            .transpose()?;
        if let Some(activation_operation) = admission.activation_operation {
            let (Some(principal_id), Some(capability_material)) =
                (admission.principal_id, capability_material.as_ref())
            else {
                return Err(RateLimitRejection::InvalidIdentity);
            };
            self.push_key(
                &mut keys,
                AdmissionDimension::Activation,
                &[
                    principal_id.as_bytes(),
                    capability_material.as_slice(),
                    activation_operation.as_bytes(),
                ],
            )?;
        }
        if let Some(session_id) = admission.session_id {
            self.push_key(
                &mut keys,
                AdmissionDimension::Session,
                &[session_id.as_bytes()],
            )?;
        }
        if let Some(operator_endpoint) = admission.operator_endpoint {
            let (Some(principal_id), Some(capability_material)) =
                (admission.principal_id, capability_material.as_ref())
            else {
                return Err(RateLimitRejection::InvalidIdentity);
            };
            self.push_key(
                &mut keys,
                AdmissionDimension::Operator,
                &[
                    principal_id.as_bytes(),
                    capability_material.as_slice(),
                    operator_endpoint.as_bytes(),
                ],
            )?;
        }
        let target_bindings = self.prepare_targets(&mut keys, principal, admission.targets)?;
        Ok(PreparedAdmission {
            keys,
            target_bindings,
        })
    }

    fn prepare_targets(
        &self,
        keys: &mut Vec<AdmissionKey>,
        principal: Option<AdmissionKey>,
        targets: &[AdmissionTarget<'_>],
    ) -> Result<Vec<PrincipalTargetKey>, RateLimitRejection> {
        let mut target_bindings = Vec::new();
        for target in targets {
            let room =
                self.push_key(keys, AdmissionDimension::Room, &[target.room_id.as_bytes()])?;
            let cardinality_target = if let Some(member_id) = target.member_id {
                self.push_key(
                    keys,
                    AdmissionDimension::Membership,
                    &[target.room_id.as_bytes(), member_id.as_bytes()],
                )?
            } else {
                room
            };
            let Some(principal) = principal else {
                return Err(RateLimitRejection::InvalidIdentity);
            };
            let binding = PrincipalTargetKey {
                principal: principal.fingerprint,
                target: cardinality_target,
            };
            if !target_bindings.contains(&binding) {
                target_bindings.push(binding);
            }
        }
        Ok(target_bindings)
    }

    fn encode_capability_material(
        material: &[u8],
    ) -> Result<[u8; CAPABILITY_HASH_BYTES * 2], RateLimitRejection> {
        const HEX: &[u8; 16] = b"0123456789abcdef";

        let material: &[u8; CAPABILITY_HASH_BYTES] = material
            .try_into()
            .map_err(|_| RateLimitRejection::InvalidIdentity)?;
        let mut encoded = [0_u8; CAPABILITY_HASH_BYTES * 2];
        for (index, byte) in material.iter().copied().enumerate() {
            encoded[index * 2] = HEX[usize::from(byte >> 4)];
            encoded[index * 2 + 1] = HEX[usize::from(byte & 0x0f)];
        }
        Ok(encoded)
    }

    fn push_key(
        &self,
        keys: &mut Vec<AdmissionKey>,
        dimension: AdmissionDimension,
        parts: &[&[u8]],
    ) -> Result<AdmissionKey, RateLimitRejection> {
        let key = self.fingerprint(dimension, parts)?;
        if keys.contains(&key) {
            return Ok(key);
        }
        if keys.len() >= MAX_KEYS_PER_ADMISSION {
            return Err(RateLimitRejection::Capacity);
        }
        keys.push(key);
        Ok(key)
    }

    /// Reserves one bounded pre-authentication `WebSocket` slot.
    pub(crate) fn reserve_websocket(&self) -> Result<PendingWebSocketPermit, RateLimitRejection> {
        let Ok(mut state) = self.inner.state.lock() else {
            self.inner.metrics.record(RateLimitRejection::Capacity);
            return Err(RateLimitRejection::Capacity);
        };
        if state.pending_websockets >= self.inner.config.max_pending_websockets {
            self.inner
                .metrics
                .record(RateLimitRejection::PendingConnectionCapacity);
            return Err(RateLimitRejection::PendingConnectionCapacity);
        }
        state.pending_websockets += 1;
        Ok(PendingWebSocketPermit {
            inner: Arc::clone(&self.inner),
            held: true,
        })
    }

    /// Fixed-label Prometheus metrics. Entity identifiers and fingerprints
    /// are intentionally absent from both metric names and label values.
    pub(crate) fn prometheus_text(&self) -> String {
        use std::fmt::Write as _;

        let mut text =
            String::from("# TYPE worldstream_gateway_rate_limit_rejections_total counter\n");
        for dimension in AdmissionDimension::ALL {
            let value = self.inner.metrics.rejections[dimension.index()].load(Ordering::Relaxed);
            let _ = writeln!(
                text,
                "worldstream_gateway_rate_limit_rejections_total{{scope=\"{}\"}} {value}",
                dimension.label()
            );
        }
        for (offset, label) in EXTRA_METRIC_LABELS.into_iter().enumerate() {
            let value = self.inner.metrics.rejections[AdmissionDimension::ALL.len() + offset]
                .load(Ordering::Relaxed);
            let _ = writeln!(
                text,
                "worldstream_gateway_rate_limit_rejections_total{{scope=\"{label}\"}} {value}"
            );
        }
        text
    }

    #[cfg(test)]
    fn for_tests(config: LimiterConfig, clock: Arc<dyn Clock>) -> Self {
        Self::with_parts(config, clock, [0x5a; 32])
    }

    #[cfg(test)]
    pub(crate) fn fixed_for_integration_tests(burst: u32) -> Self {
        Self::with_parts(
            LimiterConfig::uniform(burst, Duration::from_hours(1), 32),
            Arc::new(MonotonicClock {
                epoch: Instant::now(),
            }),
            [0x6b; 32],
        )
    }

    #[cfg(test)]
    pub(crate) fn fixed_dimension_for_integration_tests(
        dimension: AdmissionDimension,
        burst: u32,
    ) -> Self {
        let mut config = LimiterConfig::uniform(1_024, Duration::from_hours(1), 8_192);
        config.quotas[dimension.index()].burst = burst;
        Self::with_parts(
            config,
            Arc::new(MonotonicClock {
                epoch: Instant::now(),
            }),
            [0x6c; 32],
        )
    }

    #[cfg(test)]
    fn connection_counts(&self, principal: &str) -> (usize, usize, usize) {
        let fingerprint = self
            .fingerprint(AdmissionDimension::Principal, &[principal.as_bytes()])
            .map_or([0; 32], |key| key.fingerprint);
        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (
            state.pending_websockets,
            state.active_websockets,
            state
                .active_websockets_by_principal
                .get(&fingerprint)
                .copied()
                .unwrap_or(0),
        )
    }

    fn fingerprint(
        &self,
        dimension: AdmissionDimension,
        parts: &[&[u8]],
    ) -> Result<AdmissionKey, RateLimitRejection> {
        if parts.iter().any(|part| {
            part.is_empty()
                || part.len() > MAX_KEY_MATERIAL_BYTES
                || part.iter().any(u8::is_ascii_control)
        }) {
            return Err(RateLimitRejection::InvalidIdentity);
        }
        let mut hasher = blake3::Hasher::new_keyed(&self.inner.fingerprint_key);
        hasher.update(b"worldstream/gateway-rate-key/v1\0");
        hasher.update(dimension.label().as_bytes());
        for part in parts {
            let length = u64::try_from(part.len())
                .map_err(|_| RateLimitRejection::InvalidIdentity)?
                .to_be_bytes();
            hasher.update(&length);
            hasher.update(part);
        }
        Ok(AdmissionKey {
            dimension,
            fingerprint: *hasher.finalize().as_bytes(),
        })
    }
}

impl PendingWebSocketPermit {
    /// Converts a pending slot into one global and per-Principal active slot.
    pub(crate) fn activate(
        mut self,
        principal_id: &str,
    ) -> Result<ActiveWebSocketPermit, RateLimitRejection> {
        let limiter = GatewayRateLimiter {
            inner: Arc::clone(&self.inner),
        };
        let principal =
            match limiter.fingerprint(AdmissionDimension::Principal, &[principal_id.as_bytes()]) {
                Ok(key) => key.fingerprint,
                Err(rejection) => {
                    self.inner.metrics.record(rejection);
                    return Err(rejection);
                }
            };
        let Ok(mut state) = self.inner.state.lock() else {
            self.inner.metrics.record(RateLimitRejection::Capacity);
            return Err(RateLimitRejection::Capacity);
        };
        if state.active_websockets >= self.inner.config.max_active_websockets {
            self.inner
                .metrics
                .record(RateLimitRejection::ActiveConnectionCapacity);
            return Err(RateLimitRejection::ActiveConnectionCapacity);
        }
        if state
            .active_websockets_by_principal
            .get(&principal)
            .copied()
            .unwrap_or(0)
            >= self.inner.config.max_active_websockets_per_principal
        {
            self.inner
                .metrics
                .record(RateLimitRejection::PrincipalConnectionCapacity);
            return Err(RateLimitRejection::PrincipalConnectionCapacity);
        }
        state.pending_websockets = state.pending_websockets.saturating_sub(1);
        state.active_websockets += 1;
        *state
            .active_websockets_by_principal
            .entry(principal)
            .or_insert(0) += 1;
        self.held = false;
        drop(state);
        Ok(ActiveWebSocketPermit {
            inner: Arc::clone(&self.inner),
            principal,
            held: true,
        })
    }
}

impl Drop for PendingWebSocketPermit {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.pending_websockets = state.pending_websockets.saturating_sub(1);
        self.held = false;
    }
}

impl Drop for ActiveWebSocketPermit {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.active_websockets = state.active_websockets.saturating_sub(1);
        if let std::collections::hash_map::Entry::Occupied(mut count) =
            state.active_websockets_by_principal.entry(self.principal)
        {
            *count.get_mut() = count.get().saturating_sub(1);
            if *count.get() == 0 {
                count.remove();
            }
        }
        self.held = false;
    }
}

fn refilled(mut bucket: Bucket, quota: Quota, now: Duration) -> Bucket {
    let elapsed = now.saturating_sub(bucket.last_refill);
    let refill_nanos = quota.refill_every.as_nanos();
    if refill_nanos == 0 {
        return bucket;
    }
    let intervals = elapsed.as_nanos() / refill_nanos;
    if intervals == 0 {
        return bucket;
    }
    let missing = quota.burst.saturating_sub(bucket.available);
    if intervals >= u128::from(missing) {
        bucket.available = quota.burst;
        bucket.last_refill = now;
        return bucket;
    }
    let added = u32::try_from(intervals).unwrap_or(missing);
    bucket.available = bucket.available.saturating_add(added).min(quota.burst);
    let advance = quota.refill_every.checked_mul(added).unwrap_or(elapsed);
    bucket.last_refill = bucket.last_refill.checked_add(advance).unwrap_or(now);
    bucket
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Barrier,
            atomic::{AtomicU64, Ordering},
        },
        thread,
    };

    use super::*;

    #[derive(Default)]
    struct ManualClock {
        nanos: AtomicU64,
    }

    impl ManualClock {
        fn set(&self, value: Duration) {
            self.nanos.store(
                u64::try_from(value.as_nanos()).unwrap_or(u64::MAX),
                Ordering::SeqCst,
            );
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> Duration {
            Duration::from_nanos(self.nanos.load(Ordering::SeqCst))
        }
    }

    fn principal(value: &str) -> GatewayAdmission<'_> {
        GatewayAdmission {
            principal_id: Some(value),
            ..GatewayAdmission::default()
        }
    }

    #[test]
    fn exact_refill_boundary_is_deterministic() {
        let clock = Arc::new(ManualClock::default());
        let limiter = GatewayRateLimiter::for_tests(
            LimiterConfig::uniform(2, Duration::from_millis(100), 8),
            clock.clone(),
        );
        assert_eq!(limiter.admit(principal("principal-a")), Ok(()));
        assert_eq!(limiter.admit(principal("principal-a")), Ok(()));
        assert_eq!(
            limiter.admit(principal("principal-a")),
            Err(RateLimitRejection::Exhausted(AdmissionDimension::Principal))
        );
        clock.set(Duration::from_millis(99));
        assert!(limiter.admit(principal("principal-a")).is_err());
        clock.set(Duration::from_millis(100));
        assert_eq!(limiter.admit(principal("principal-a")), Ok(()));
        assert!(limiter.admit(principal("principal-a")).is_err());
        clock.set(Duration::from_millis(300));
        assert_eq!(limiter.admit(principal("principal-a")), Ok(()));
        assert_eq!(limiter.admit(principal("principal-a")), Ok(()));
    }

    #[test]
    fn multi_dimension_rejection_does_not_partially_consume_other_buckets() {
        let clock = Arc::new(ManualClock::default());
        let limiter = GatewayRateLimiter::for_tests(
            LimiterConfig::uniform(1, Duration::from_secs(1), 8),
            clock,
        );
        let target = [AdmissionTarget {
            room_id: "room-a",
            member_id: None,
        }];
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-a"),
                targets: &target,
                ..GatewayAdmission::default()
            }),
            Ok(())
        );
        let target_b = [AdmissionTarget {
            room_id: "room-b",
            member_id: None,
        }];
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-a"),
                targets: &target_b,
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::Exhausted(AdmissionDimension::Principal))
        );
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-b"),
                targets: &target_b,
                ..GatewayAdmission::default()
            }),
            Ok(())
        );
    }

    #[test]
    fn memberships_are_composite_room_member_keys() {
        let clock = Arc::new(ManualClock::default());
        let limiter = GatewayRateLimiter::for_tests(
            LimiterConfig::uniform(2, Duration::from_secs(1), 8),
            clock,
        );
        for room_id in ["room-a", "room-b"] {
            let target = [AdmissionTarget {
                room_id,
                member_id: Some("member-a"),
            }];
            assert_eq!(
                limiter.admit(GatewayAdmission {
                    principal_id: Some("principal-a"),
                    targets: &target,
                    ..GatewayAdmission::default()
                }),
                Ok(())
            );
        }
    }

    #[test]
    fn per_dimension_capacity_fails_closed_and_idle_entries_expire() {
        let clock = Arc::new(ManualClock::default());
        let mut config = LimiterConfig::uniform(1, Duration::from_millis(10), 1);
        config.idle_retention = Duration::from_millis(100);
        let limiter = GatewayRateLimiter::for_tests(config, clock.clone());
        assert_eq!(limiter.admit(principal("principal-a")), Ok(()));
        assert_eq!(
            limiter.admit(principal("principal-b")),
            Err(RateLimitRejection::Capacity)
        );
        clock.set(Duration::from_millis(100));
        assert_eq!(limiter.admit(principal("principal-b")), Ok(()));
    }

    #[test]
    fn clock_regression_and_invalid_material_fail_closed() {
        let clock = Arc::new(ManualClock::default());
        let limiter = GatewayRateLimiter::for_tests(
            LimiterConfig::uniform(2, Duration::from_millis(10), 8),
            clock.clone(),
        );
        clock.set(Duration::from_millis(20));
        assert_eq!(limiter.admit(principal("principal-a")), Ok(()));
        clock.set(Duration::from_millis(19));
        assert_eq!(
            limiter.admit(principal("principal-a")),
            Err(RateLimitRejection::Clock)
        );
        assert_eq!(
            limiter.admit(principal("bad\nprincipal")),
            Err(RateLimitRejection::InvalidIdentity)
        );
    }

    #[test]
    fn metrics_have_only_fixed_labels_and_never_contain_key_material() {
        let clock = Arc::new(ManualClock::default());
        let limiter = GatewayRateLimiter::for_tests(
            LimiterConfig::uniform(1, Duration::from_secs(1), 8),
            clock,
        );
        let private_principal = "private-principal-01ARZ3NDEKTSV4RRFFQ69G5FAV";
        assert_eq!(limiter.admit(principal(private_principal)), Ok(()));
        assert!(limiter.admit(principal(private_principal)).is_err());
        let metrics = limiter.prometheus_text();
        assert!(metrics.contains("scope=\"principal\"} 1"));
        for label in AdmissionDimension::ALL
            .into_iter()
            .map(AdmissionDimension::label)
            .chain(EXTRA_METRIC_LABELS)
        {
            assert!(metrics.contains(&format!("scope=\"{label}\"}}")));
        }
        assert!(!metrics.contains(private_principal));
        assert!(!metrics.contains("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
    }

    #[test]
    fn binary_capability_hashes_are_admitted_and_shared_across_transport_scopes() {
        let clock = Arc::new(ManualClock::default());
        let limiter = GatewayRateLimiter::for_tests(
            LimiterConfig::uniform(1, Duration::from_secs(1), 8),
            clock,
        );
        let capability = [0x09_u8; CAPABILITY_HASH_BYTES];
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("websocket-principal"),
                capability_material: Some(&capability),
                session_id: Some("websocket-session"),
                ..GatewayAdmission::default()
            }),
            Ok(())
        );
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("http-principal"),
                capability_material: Some(&capability),
                operator_endpoint: Some("room-create"),
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::Exhausted(
                AdmissionDimension::Capability
            ))
        );
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("bad-capability"),
                capability_material: Some(&capability[..CAPABILITY_HASH_BYTES - 1]),
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::InvalidIdentity)
        );
    }

    #[test]
    fn activation_and_operator_keys_are_principal_capability_operation_composites() {
        let clock = Arc::new(ManualClock::default());
        let mut config = LimiterConfig::uniform(8, Duration::from_secs(1), 64);
        config.quotas[AdmissionDimension::Activation.index()].burst = 1;
        config.quotas[AdmissionDimension::Operator.index()].burst = 1;
        let limiter = GatewayRateLimiter::for_tests(config, clock);
        let capability = [0x11_u8; CAPABILITY_HASH_BYTES];

        let activation = |principal_id, operation| GatewayAdmission {
            principal_id: Some(principal_id),
            capability_material: Some(&capability),
            activation_operation: Some(operation),
            ..GatewayAdmission::default()
        };
        assert_eq!(
            limiter.admit(activation("principal-a", "activation.claim")),
            Ok(())
        );
        assert_eq!(
            limiter.admit(activation("principal-a", "activation.claim")),
            Err(RateLimitRejection::Exhausted(
                AdmissionDimension::Activation
            ))
        );
        assert_eq!(
            limiter.admit(activation("principal-a", "activation.renew")),
            Ok(())
        );
        assert_eq!(
            limiter.admit(activation("principal-b", "activation.claim")),
            Ok(())
        );

        let operator = |principal_id, endpoint| GatewayAdmission {
            principal_id: Some(principal_id),
            capability_material: Some(&capability),
            operator_endpoint: Some(endpoint),
            ..GatewayAdmission::default()
        };
        assert_eq!(
            limiter.admit(operator("principal-c", "room-create")),
            Ok(())
        );
        assert_eq!(
            limiter.admit(operator("principal-c", "room-create")),
            Err(RateLimitRejection::Exhausted(AdmissionDimension::Operator))
        );
        assert_eq!(limiter.admit(operator("principal-c", "timer-fire")), Ok(()));
        assert_eq!(
            limiter.admit(operator("principal-d", "room-create")),
            Ok(())
        );
    }

    #[test]
    fn target_bindings_require_a_principal_and_bound_each_principal() {
        let clock = Arc::new(ManualClock::default());
        let mut config = LimiterConfig::uniform(16, Duration::from_secs(1), 64);
        config.max_targets_per_principal = 2;
        config.max_target_bindings = 8;
        let limiter = GatewayRateLimiter::for_tests(config, clock);
        let first = [AdmissionTarget {
            room_id: "room-a",
            member_id: Some("member-a"),
        }];
        assert_eq!(
            limiter.admit(GatewayAdmission {
                targets: &first,
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::InvalidIdentity)
        );
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-a"),
                targets: &first,
                ..GatewayAdmission::default()
            }),
            Ok(())
        );
        let second = [AdmissionTarget {
            room_id: "room-b",
            member_id: Some("member-b"),
        }];
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-a"),
                targets: &second,
                ..GatewayAdmission::default()
            }),
            Ok(())
        );
        let third = [AdmissionTarget {
            room_id: "room-c",
            member_id: Some("member-c"),
        }];
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-a"),
                targets: &third,
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::TargetCapacity)
        );
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-b"),
                targets: &third,
                ..GatewayAdmission::default()
            }),
            Ok(())
        );
    }

    #[test]
    fn target_cardinality_accepts_exactly_256_live_targets_per_principal() {
        let clock = Arc::new(ManualClock::default());
        let mut config = LimiterConfig::uniform(300, Duration::from_secs(1), 1_024);
        config.max_targets_per_principal = MAX_TARGETS_PER_PRINCIPAL;
        config.max_target_bindings = 1_024;
        let limiter = GatewayRateLimiter::for_tests(config, clock);

        for index in 0..MAX_TARGETS_PER_PRINCIPAL {
            let room_id = format!("room-{index}");
            let member_id = format!("member-{index}");
            let target = [AdmissionTarget {
                room_id: &room_id,
                member_id: Some(&member_id),
            }];
            assert_eq!(
                limiter.admit(GatewayAdmission {
                    principal_id: Some("principal-a"),
                    targets: &target,
                    ..GatewayAdmission::default()
                }),
                Ok(())
            );
        }
        let overflow = [AdmissionTarget {
            room_id: "room-overflow",
            member_id: Some("member-overflow"),
        }];
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-a"),
                targets: &overflow,
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::TargetCapacity)
        );
    }

    #[test]
    fn global_target_binding_capacity_contains_new_key_poisoning() {
        let clock = Arc::new(ManualClock::default());
        let mut config = LimiterConfig::uniform(8, Duration::from_secs(1), 64);
        config.max_target_bindings = 2;
        let limiter = GatewayRateLimiter::for_tests(config, clock);
        let targets = [
            AdmissionTarget {
                room_id: "room-a",
                member_id: Some("member-a"),
            },
            AdmissionTarget {
                room_id: "room-b",
                member_id: Some("member-b"),
            },
            AdmissionTarget {
                room_id: "room-c",
                member_id: Some("member-c"),
            },
        ];
        for (principal_id, target) in ["principal-a", "principal-b"]
            .into_iter()
            .zip(&targets[..2])
        {
            assert_eq!(
                limiter.admit(GatewayAdmission {
                    principal_id: Some(principal_id),
                    targets: std::slice::from_ref(target),
                    ..GatewayAdmission::default()
                }),
                Ok(())
            );
        }
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-c"),
                targets: &targets[2..],
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::TargetCapacity)
        );
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-a"),
                targets: &targets[..1],
                ..GatewayAdmission::default()
            }),
            Ok(())
        );
    }

    #[test]
    fn quota_rejection_refreshes_existing_target_ownership() {
        let clock = Arc::new(ManualClock::default());
        let mut config = LimiterConfig::uniform(8, Duration::from_secs(1), 64);
        config.quotas[AdmissionDimension::Membership.index()].burst = 1;
        config.idle_retention = Duration::from_millis(100);
        config.max_targets_per_principal = 1;
        let limiter = GatewayRateLimiter::for_tests(config, clock.clone());
        let existing = [AdmissionTarget {
            room_id: "room-a",
            member_id: Some("member-a"),
        }];
        let existing_admission = || GatewayAdmission {
            principal_id: Some("principal-a"),
            targets: &existing,
            ..GatewayAdmission::default()
        };
        assert_eq!(limiter.admit(existing_admission()), Ok(()));
        clock.set(Duration::from_millis(50));
        assert_eq!(
            limiter.admit(existing_admission()),
            Err(RateLimitRejection::Exhausted(
                AdmissionDimension::Membership
            ))
        );
        clock.set(Duration::from_millis(100));
        let replacement = [AdmissionTarget {
            room_id: "room-b",
            member_id: Some("member-b"),
        }];
        assert_eq!(
            limiter.admit(GatewayAdmission {
                principal_id: Some("principal-a"),
                targets: &replacement,
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::TargetCapacity)
        );
    }

    #[test]
    fn concurrent_admission_accepts_exactly_one_burst() {
        const BURST: usize = 32;
        const CONTENDERS: usize = BURST * 2;
        let clock = Arc::new(ManualClock::default());
        let limiter = Arc::new(GatewayRateLimiter::for_tests(
            LimiterConfig::uniform(32, Duration::from_secs(1), 8),
            clock,
        ));
        let barrier = Arc::new(Barrier::new(CONTENDERS));
        let handles = (0..CONTENDERS)
            .map(|_| {
                let limiter = Arc::clone(&limiter);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    limiter.admit(principal("principal-a"))
                })
            })
            .collect::<Vec<_>>();
        let outcomes = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| unreachable!("admission thread"))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
            BURST
        );
        assert!(
            outcomes
                .iter()
                .filter(|outcome| outcome.is_err())
                .all(|outcome| {
                    *outcome == Err(RateLimitRejection::Exhausted(AdmissionDimension::Principal))
                })
        );
    }

    #[test]
    fn websocket_permits_enforce_pending_global_and_principal_caps_with_raii() {
        let clock = Arc::new(ManualClock::default());
        let mut config = LimiterConfig::uniform(8, Duration::from_secs(1), 8);
        config.max_pending_websockets = 2;
        config.max_active_websockets = 2;
        config.max_active_websockets_per_principal = 1;
        let limiter = GatewayRateLimiter::for_tests(config, clock);

        let pending_a = limiter
            .reserve_websocket()
            .unwrap_or_else(|error| unreachable!("pending A: {error:?}"));
        let pending_same_principal = limiter
            .reserve_websocket()
            .unwrap_or_else(|error| unreachable!("pending duplicate: {error:?}"));
        assert!(matches!(
            limiter.reserve_websocket(),
            Err(RateLimitRejection::PendingConnectionCapacity)
        ));
        let active_a = pending_a
            .activate("principal-a")
            .unwrap_or_else(|error| unreachable!("active A: {error:?}"));
        assert_eq!(limiter.connection_counts("principal-a"), (1, 1, 1));
        assert!(matches!(
            pending_same_principal.activate("principal-a"),
            Err(RateLimitRejection::PrincipalConnectionCapacity)
        ));
        assert_eq!(limiter.connection_counts("principal-a"), (0, 1, 1));

        let active_b = limiter
            .reserve_websocket()
            .unwrap_or_else(|error| unreachable!("pending B: {error:?}"))
            .activate("principal-b")
            .unwrap_or_else(|error| unreachable!("active B: {error:?}"));
        assert_eq!(limiter.connection_counts("principal-b"), (0, 2, 1));
        assert!(matches!(
            limiter
                .reserve_websocket()
                .unwrap_or_else(|error| unreachable!("pending C: {error:?}"))
                .activate("principal-c"),
            Err(RateLimitRejection::ActiveConnectionCapacity)
        ));
        drop(active_a);
        assert_eq!(limiter.connection_counts("principal-a"), (0, 1, 0));
        drop(active_b);
        assert_eq!(limiter.connection_counts("principal-b"), (0, 0, 0));
    }

    #[test]
    fn mapped_ipv6_and_ipv4_share_one_peer_bucket() {
        let clock = Arc::new(ManualClock::default());
        let limiter = GatewayRateLimiter::for_tests(
            LimiterConfig::uniform(1, Duration::from_secs(1), 8),
            clock,
        );
        let ipv4 = PeerIdentity::from_ip(Some(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)));
        let mapped = PeerIdentity::from_ip(Some(IpAddr::V6(
            std::net::Ipv4Addr::LOCALHOST.to_ipv6_mapped(),
        )));
        assert_eq!(
            limiter.admit(GatewayAdmission {
                peer: Some(ipv4),
                ..GatewayAdmission::default()
            }),
            Ok(())
        );
        assert_eq!(
            limiter.admit(GatewayAdmission {
                peer: Some(mapped),
                ..GatewayAdmission::default()
            }),
            Err(RateLimitRejection::Exhausted(AdmissionDimension::Ip))
        );
    }
}

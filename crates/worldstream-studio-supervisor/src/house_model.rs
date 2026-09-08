//! Bounded model-provider boundary and durable allowance ledger for House Agents.
//!
//! The module deliberately knows nothing about Room authority. It accepts only
//! an immutable House Agent Revision, one stable Invocation identity, and the
//! already-authorized Projection and Action Offers. Provider input and output
//! are short-lived, zeroized buffers; durable state contains only bounded usage
//! evidence and hashes.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read as _, Write as _},
    net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value, json};
use thiserror::Error;
use time::OffsetDateTime;
use worldstream_core::CanonicalJsonV1;
use worldstream_hosted_contract::{HouseAgentCompletionTokenParameterV1, HouseAgentRevision};
use worldstream_runtime::{
    create_owner_only_file, create_owner_only_renameable_file, prepare_data_directory,
    validate_owner_only_file,
};
use zeroize::Zeroizing;

use crate::assignment_mcp_actions::{
    action_payload_matches_schema_v1, action_payload_schema_is_valid_v1,
};

const LEDGER_SCHEMA_V1: &str = "worldstream/house-allowance-ledger@1";
const ACCOUNTING_TOKENIZER_ID: &str = "worldstream.utf8-byte-accounting";
const ACCOUNTING_TOKENIZER_REVISION: &str = "1";
const OPENROUTER_AUTHORITY: &str = "openrouter.ai";
const OPENROUTER_PORT: u16 = 443;
const OPENROUTER_PATH: &str = "/api/v1/chat/completions";
const MAX_LEDGER_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_ASSIGNMENTS: usize = 256;
const MAX_ATTEMPTS_PER_ASSIGNMENT: usize = 10;
const MAX_IDENTITY_BYTES: usize = 256;
const MAX_GENERATION_ID_BYTES: usize = 256;
const NANODOLLARS_PER_DOLLAR: u64 = 1_000_000_000;
const OPENROUTER_PRICE_SCALE: usize = 6;

/// Aggregate operator-funded blast-radius limits for House model calls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HouseSpendLimitsV1 {
    pub daily_nano_usd: u64,
    pub monthly_nano_usd: u64,
}

impl HouseSpendLimitsV1 {
    /// Frozen hobby-preview limits: USD 2/day and USD 10/month.
    #[must_use]
    pub const fn hobby_preview() -> Self {
        Self {
            daily_nano_usd: 2 * NANODOLLARS_PER_DOLLAR,
            monthly_nano_usd: 10 * NANODOLLARS_PER_DOLLAR,
        }
    }

    fn valid(self) -> bool {
        self.daily_nano_usd > 0 && self.monthly_nano_usd >= self.daily_nano_usd
    }
}

impl Default for HouseSpendLimitsV1 {
    fn default() -> Self {
        Self::hobby_preview()
    }
}

/// UTC accounting buckets supplied to an atomic ledger mutation.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HouseAllowancePeriodV1 {
    day_index: u64,
    month_index: u32,
}

impl HouseAllowancePeriodV1 {
    /// Derives stable UTC day and month buckets from Unix time.
    ///
    /// # Errors
    ///
    /// Rejects timestamps outside the supported calendar or before Unix epoch.
    pub fn from_unix_seconds(seconds: i64) -> Result<Self, HouseAllowanceErrorV1> {
        let timestamp = OffsetDateTime::from_unix_timestamp(seconds)
            .map_err(|_| HouseAllowanceErrorV1::InvalidInput)?;
        let day_index = u64::try_from(seconds.div_euclid(86_400))
            .map_err(|_| HouseAllowanceErrorV1::InvalidInput)?;
        let month = u32::from(u8::from(timestamp.month()));
        let year =
            u32::try_from(timestamp.year()).map_err(|_| HouseAllowanceErrorV1::InvalidInput)?;
        let month_index = year
            .checked_mul(12)
            .and_then(|value| value.checked_add(month - 1))
            .ok_or(HouseAllowanceErrorV1::InvalidInput)?;
        Ok(Self {
            day_index,
            month_index,
        })
    }

    /// Reads the current UTC accounting buckets.
    ///
    /// # Errors
    ///
    /// Returns a closed unavailable error when the system clock is invalid.
    pub fn now() -> Result<Self, HouseAllowanceErrorV1> {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| HouseAllowanceErrorV1::Unavailable)?
            .as_secs();
        let seconds = i64::try_from(seconds).map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
        Self::from_unix_seconds(seconds)
    }
}

/// Exact immutable identity for one possible provider dispatch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HouseInvocationIdentityV1 {
    assignment_id: String,
    attempt_digest: String,
}

impl HouseInvocationIdentityV1 {
    /// Constructs the stable identity used for at-most-once model dispatch.
    ///
    /// # Errors
    ///
    /// Rejects empty, unbounded, control-bearing, or non-digest values.
    pub fn new(
        assignment_id: &str,
        activation_id: &str,
        lease_generation: u64,
        context_digest: &str,
    ) -> Result<Self, HouseAllowanceErrorV1> {
        if !bounded_identity(assignment_id)
            || !bounded_identity(activation_id)
            || lease_generation == 0
            || !valid_blake3_digest(context_digest)
        {
            return Err(HouseAllowanceErrorV1::InvalidInput);
        }
        Ok(Self {
            assignment_id: assignment_id.to_owned(),
            attempt_digest: house_provider_attempt_digest(
                assignment_id,
                activation_id,
                lease_generation,
                context_digest,
            ),
        })
    }

    /// Constructs the same ledger identity from the opaque digest emitted by
    /// the authority-holding assignment helper. This keeps Activation,
    /// Membership, and Room identifiers out of the model-host process.
    ///
    /// # Errors
    /// Rejects an invalid unit scope or digest.
    pub fn from_sealed_attempt(
        assignment_scope: &str,
        attempt_digest: &str,
    ) -> Result<Self, HouseAllowanceErrorV1> {
        if !bounded_identity(assignment_scope) || !valid_blake3_digest(attempt_digest) {
            return Err(HouseAllowanceErrorV1::InvalidInput);
        }
        Ok(Self {
            assignment_id: assignment_scope.to_owned(),
            attempt_digest: attempt_digest.to_owned(),
        })
    }

    #[must_use]
    pub fn assignment_id(&self) -> &str {
        &self.assignment_id
    }

    #[must_use]
    pub fn attempt_digest(&self) -> String {
        self.attempt_digest.clone()
    }
}

/// Derives the opaque provider-attempt identity inside the authority-holding
/// helper before any data crosses into the model host.
#[must_use]
pub(crate) fn house_provider_attempt_digest(
    assignment_id: &str,
    activation_id: &str,
    lease_generation: u64,
    context_digest: &str,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"worldstream/house-provider-attempt/v1\0");
    hash_field(&mut hasher, assignment_id.as_bytes());
    hash_field(&mut hasher, activation_id.as_bytes());
    hash_field(&mut hasher, &lease_generation.to_be_bytes());
    hash_field(&mut hasher, context_digest.as_bytes());
    format!("blake3:{}", hasher.finalize().to_hex())
}

/// Closed durable-admission failures. They contain no prompt or provider text.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HouseAllowanceErrorV1 {
    #[error("House allowance input is invalid")]
    InvalidInput,
    #[error("House allowance storage is unavailable")]
    Unavailable,
    #[error("House allowance storage is corrupt")]
    Corrupt,
    #[error("House assignment is disabled")]
    AssignmentDisabled,
    #[error("House assignment capacity is exhausted")]
    AssignmentCapacity,
    #[error("House model attempt was already consumed")]
    AttemptConsumed,
    #[error("House assignment already has an active model call")]
    ConcurrentCall,
    #[error("House assignment model-call limit is exhausted")]
    CallLimit,
    #[error("House assignment token limit is exhausted")]
    TokenLimit,
    #[error("aggregate House model spend limit is exhausted")]
    SpendLimit,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum AttemptStateV1 {
    Reserved,
    Completed,
    Ambiguous,
    ProviderFailed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct AttemptRecordV1 {
    state: AttemptStateV1,
    input_units: u64,
    output_units: u64,
    maximum_output_units: u64,
    reserved_nano_usd: u64,
    response_digest: Option<String>,
    provider_prompt_tokens: Option<u64>,
    provider_completion_tokens: Option<u64>,
    provider_cost_nano_usd: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct AssignmentRecordV1 {
    revision_digest: String,
    disabled: bool,
    consumed_input_units: u64,
    consumed_output_units: u64,
    attempts: BTreeMap<String, AttemptRecordV1>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct LedgerRecordV1 {
    schema: String,
    day_index: u64,
    month_index: u32,
    daily_reserved_nano_usd: u64,
    monthly_reserved_nano_usd: u64,
    assignments: BTreeMap<String, AssignmentRecordV1>,
    integrity_hash: String,
}

#[derive(Serialize)]
struct LedgerIntegrityWitnessV1<'a> {
    schema: &'a str,
    day_index: u64,
    month_index: u32,
    daily_reserved_nano_usd: u64,
    monthly_reserved_nano_usd: u64,
    assignments: &'a BTreeMap<String, AssignmentRecordV1>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HouseAllowanceReservationV1 {
    assignment_id: String,
    attempt_digest: String,
}

/// Safe bounded usage projection for operator diagnostics and tests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HouseAllowanceUsageV1 {
    pub attempts: u64,
    pub consumed_input_units: u64,
    pub consumed_output_units: u64,
    pub active_calls: u64,
    pub daily_reserved_nano_usd: u64,
    pub monthly_reserved_nano_usd: u64,
}

/// Owner-only, atomically replaced allowance ledger suitable for one Fly volume.
#[derive(Clone)]
pub struct FileHouseAllowanceLedgerV1 {
    root: Arc<PathBuf>,
    state_path: Arc<PathBuf>,
    process_lock: Arc<fs::File>,
    mutation: Arc<Mutex<()>>,
    limits: HouseSpendLimitsV1,
}

struct ProcessFileLockV1<'a>(&'a fs::File);

impl Drop for ProcessFileLockV1<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

impl FileHouseAllowanceLedgerV1 {
    /// Opens the production ledger at the current UTC period.
    ///
    /// # Errors
    ///
    /// Rejects unsafe storage, corrupt retained data, and invalid limits.
    pub fn open(
        root: impl AsRef<Path>,
        limits: HouseSpendLimitsV1,
    ) -> Result<Self, HouseAllowanceErrorV1> {
        Self::open_at_scope(root, limits, HouseAllowancePeriodV1::now()?, None)
    }

    /// Opens the shared production ledger while recovering only one exact
    /// House Runner unit's interrupted reservation.
    ///
    /// Other units can still have provider calls in flight when a new or
    /// restarted unit opens the same aggregate ledger. Their reservations
    /// therefore remain untouched.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe assignment scope, unsafe storage, corrupt retained
    /// data, and invalid limits.
    pub fn open_for_assignment(
        root: impl AsRef<Path>,
        limits: HouseSpendLimitsV1,
        assignment_scope: &str,
    ) -> Result<Self, HouseAllowanceErrorV1> {
        if !bounded_identity(assignment_scope) {
            return Err(HouseAllowanceErrorV1::InvalidInput);
        }
        Self::open_at_scope(
            root,
            limits,
            HouseAllowancePeriodV1::now()?,
            Some(assignment_scope),
        )
    }

    /// Opens the ledger at an explicit UTC period for deterministic recovery.
    ///
    /// # Errors
    ///
    /// Rejects unsafe storage, corrupt retained data, clock rollback, and invalid limits.
    pub fn open_at(
        root: impl AsRef<Path>,
        limits: HouseSpendLimitsV1,
        period: HouseAllowancePeriodV1,
    ) -> Result<Self, HouseAllowanceErrorV1> {
        Self::open_at_scope(root, limits, period, None)
    }

    fn open_at_scope(
        root: impl AsRef<Path>,
        limits: HouseSpendLimitsV1,
        period: HouseAllowancePeriodV1,
        assignment_scope: Option<&str>,
    ) -> Result<Self, HouseAllowanceErrorV1> {
        if !limits.valid() {
            return Err(HouseAllowanceErrorV1::InvalidInput);
        }
        let root = prepare_data_directory(root.as_ref())
            .map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
        let lock_path = root.join(".allowance-ledger.lock");
        let process_lock = match create_owner_only_file(&lock_path) {
            Ok(file) => file,
            Err(_) if lock_path.exists() => fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|_| HouseAllowanceErrorV1::Unavailable)?,
            Err(_) => return Err(HouseAllowanceErrorV1::Unavailable),
        };
        validate_owner_only_file(&lock_path).map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
        let ledger = Self {
            state_path: Arc::new(root.join("allowances.json")),
            root: Arc::new(root),
            process_lock: Arc::new(process_lock),
            mutation: Arc::new(Mutex::new(())),
            limits,
        };
        let mutation = ledger.mutation.lock().map_err(map_poison)?;
        let process = ledger.lock_process()?;
        ledger.remove_stale_temporaries()?;
        let mut record = ledger.load_or_empty(period)?;
        let mut changed = roll_period(&mut record, period)?;
        for (assignment_id, assignment) in &mut record.assignments {
            if assignment_scope.is_none_or(|scope| scope == assignment_id) {
                for attempt in assignment.attempts.values_mut() {
                    if attempt.state == AttemptStateV1::Reserved {
                        attempt.state = AttemptStateV1::Ambiguous;
                        changed = true;
                    }
                }
            }
        }
        if changed || !ledger.state_path.exists() {
            ledger.persist(&mut record)?;
        }
        drop(process);
        drop(mutation);
        Ok(ledger)
    }

    fn lock_process(&self) -> Result<ProcessFileLockV1<'_>, HouseAllowanceErrorV1> {
        self.process_lock
            .lock()
            .map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
        Ok(ProcessFileLockV1(&self.process_lock))
    }

    fn reserve(
        &self,
        identity: &HouseInvocationIdentityV1,
        revision: &HouseAgentRevision,
        input_units: u64,
        period: HouseAllowancePeriodV1,
    ) -> Result<HouseAllowanceReservationV1, HouseAllowanceErrorV1> {
        let allowance = revision.allowance();
        if input_units == 0 || input_units > allowance.input_tokens_per_call {
            return Err(HouseAllowanceErrorV1::TokenLimit);
        }
        let prompt_price = decimal_usd_to_nano_usd(revision.maximum_prompt_price())?;
        let completion_price = decimal_usd_to_nano_usd(revision.maximum_completion_price())?;
        let reserved_nano_usd = checked_cost(
            input_units,
            prompt_price,
            allowance.output_tokens_per_call,
            completion_price,
        )?;
        let attempt_digest = identity.attempt_digest();
        let _mutation = self.mutation.lock().map_err(map_poison)?;
        let _process = self.lock_process()?;
        let mut ledger = self.load()?;
        roll_period(&mut ledger, period)?;
        if ledger.assignments.len() >= MAX_ASSIGNMENTS
            && !ledger.assignments.contains_key(identity.assignment_id())
        {
            return Err(HouseAllowanceErrorV1::AssignmentCapacity);
        }
        let assignment = ledger
            .assignments
            .entry(identity.assignment_id().to_owned())
            .or_insert_with(|| AssignmentRecordV1 {
                revision_digest: revision.digest().to_owned(),
                disabled: false,
                consumed_input_units: 0,
                consumed_output_units: 0,
                attempts: BTreeMap::new(),
            });
        validate_assignment_for_revision(assignment, revision)?;
        if assignment.disabled {
            return Err(HouseAllowanceErrorV1::AssignmentDisabled);
        }
        if assignment.attempts.contains_key(&attempt_digest) {
            return Err(HouseAllowanceErrorV1::AttemptConsumed);
        }
        if assignment
            .attempts
            .values()
            .filter(|attempt| attempt.state == AttemptStateV1::Reserved)
            .count()
            >= usize::try_from(allowance.concurrent_calls).unwrap_or(usize::MAX)
        {
            return Err(HouseAllowanceErrorV1::ConcurrentCall);
        }
        if assignment.attempts.len()
            >= usize::try_from(allowance.model_call_attempts).unwrap_or(usize::MAX)
        {
            return Err(HouseAllowanceErrorV1::CallLimit);
        }
        let next_input = assignment
            .consumed_input_units
            .checked_add(input_units)
            .ok_or(HouseAllowanceErrorV1::TokenLimit)?;
        let next_output = assignment
            .consumed_output_units
            .checked_add(allowance.output_tokens_per_call)
            .ok_or(HouseAllowanceErrorV1::TokenLimit)?;
        if next_input > allowance.total_input_tokens || next_output > allowance.total_output_tokens
        {
            return Err(HouseAllowanceErrorV1::TokenLimit);
        }
        let next_daily = ledger
            .daily_reserved_nano_usd
            .checked_add(reserved_nano_usd)
            .ok_or(HouseAllowanceErrorV1::SpendLimit)?;
        let next_monthly = ledger
            .monthly_reserved_nano_usd
            .checked_add(reserved_nano_usd)
            .ok_or(HouseAllowanceErrorV1::SpendLimit)?;
        if next_daily > self.limits.daily_nano_usd || next_monthly > self.limits.monthly_nano_usd {
            return Err(HouseAllowanceErrorV1::SpendLimit);
        }
        assignment.consumed_input_units = next_input;
        assignment.consumed_output_units = next_output;
        assignment.attempts.insert(
            attempt_digest.clone(),
            AttemptRecordV1 {
                state: AttemptStateV1::Reserved,
                input_units,
                output_units: allowance.output_tokens_per_call,
                maximum_output_units: allowance.output_tokens_per_call,
                reserved_nano_usd,
                response_digest: None,
                provider_prompt_tokens: None,
                provider_completion_tokens: None,
                provider_cost_nano_usd: None,
            },
        );
        ledger.daily_reserved_nano_usd = next_daily;
        ledger.monthly_reserved_nano_usd = next_monthly;
        self.persist(&mut ledger)?;
        Ok(HouseAllowanceReservationV1 {
            assignment_id: identity.assignment_id().to_owned(),
            attempt_digest,
        })
    }

    fn complete(
        &self,
        reservation: &HouseAllowanceReservationV1,
        output_units: u64,
        evidence: &HouseProviderEvidenceV1,
    ) -> Result<(), HouseAllowanceErrorV1> {
        let _mutation = self.mutation.lock().map_err(map_poison)?;
        let _process = self.lock_process()?;
        let mut ledger = self.load()?;
        let assignment = ledger
            .assignments
            .get_mut(&reservation.assignment_id)
            .ok_or(HouseAllowanceErrorV1::Corrupt)?;
        let attempt = assignment
            .attempts
            .get_mut(&reservation.attempt_digest)
            .ok_or(HouseAllowanceErrorV1::Corrupt)?;
        if attempt.state != AttemptStateV1::Reserved
            || output_units > attempt.maximum_output_units
            || attempt.output_units != attempt.maximum_output_units
        {
            return Err(HouseAllowanceErrorV1::AttemptConsumed);
        }
        let released = attempt.maximum_output_units - output_units;
        assignment.consumed_output_units = assignment
            .consumed_output_units
            .checked_sub(released)
            .ok_or(HouseAllowanceErrorV1::Corrupt)?;
        attempt.state = AttemptStateV1::Completed;
        attempt.output_units = output_units;
        attempt.response_digest = Some(evidence.response_digest.clone());
        attempt.provider_prompt_tokens = Some(evidence.provider_prompt_tokens);
        attempt.provider_completion_tokens = Some(evidence.provider_completion_tokens);
        attempt.provider_cost_nano_usd = evidence.provider_cost_nano_usd;
        self.persist(&mut ledger)
    }

    fn consume_failed(
        &self,
        reservation: &HouseAllowanceReservationV1,
        state: AttemptStateV1,
    ) -> Result<(), HouseAllowanceErrorV1> {
        if !matches!(
            state,
            AttemptStateV1::Ambiguous | AttemptStateV1::ProviderFailed
        ) {
            return Err(HouseAllowanceErrorV1::InvalidInput);
        }
        let _mutation = self.mutation.lock().map_err(map_poison)?;
        let _process = self.lock_process()?;
        let mut ledger = self.load()?;
        let attempt = ledger
            .assignments
            .get_mut(&reservation.assignment_id)
            .and_then(|assignment| assignment.attempts.get_mut(&reservation.attempt_digest))
            .ok_or(HouseAllowanceErrorV1::Corrupt)?;
        if attempt.state != AttemptStateV1::Reserved {
            return Err(HouseAllowanceErrorV1::AttemptConsumed);
        }
        attempt.state = state;
        self.persist(&mut ledger)
    }

    /// Disables every restored assignment before a recovered deployment serves calls.
    ///
    /// # Errors
    ///
    /// Returns a closed storage error if the disable fence cannot be persisted.
    pub fn disable_all_after_restore(&self) -> Result<(), HouseAllowanceErrorV1> {
        let _mutation = self.mutation.lock().map_err(map_poison)?;
        let _process = self.lock_process()?;
        let mut ledger = self.load()?;
        for assignment in ledger.assignments.values_mut() {
            assignment.disabled = true;
            for attempt in assignment.attempts.values_mut() {
                if attempt.state == AttemptStateV1::Reserved {
                    attempt.state = AttemptStateV1::Ambiguous;
                }
            }
        }
        self.persist(&mut ledger)
    }

    /// Returns only safe counters for one assignment and the active spend buckets.
    ///
    /// # Errors
    ///
    /// Returns a closed storage error for absent, unsafe, or corrupt state.
    pub fn usage(
        &self,
        assignment_id: &str,
    ) -> Result<HouseAllowanceUsageV1, HouseAllowanceErrorV1> {
        if !bounded_identity(assignment_id) {
            return Err(HouseAllowanceErrorV1::InvalidInput);
        }
        let _mutation = self.mutation.lock().map_err(map_poison)?;
        let _process = self.lock_process()?;
        let ledger = self.load()?;
        let assignment = ledger
            .assignments
            .get(assignment_id)
            .ok_or(HouseAllowanceErrorV1::InvalidInput)?;
        Ok(HouseAllowanceUsageV1 {
            attempts: u64::try_from(assignment.attempts.len()).unwrap_or(u64::MAX),
            consumed_input_units: assignment.consumed_input_units,
            consumed_output_units: assignment.consumed_output_units,
            active_calls: u64::try_from(
                assignment
                    .attempts
                    .values()
                    .filter(|attempt| attempt.state == AttemptStateV1::Reserved)
                    .count(),
            )
            .unwrap_or(u64::MAX),
            daily_reserved_nano_usd: ledger.daily_reserved_nano_usd,
            monthly_reserved_nano_usd: ledger.monthly_reserved_nano_usd,
        })
    }

    fn remove_stale_temporaries(&self) -> Result<(), HouseAllowanceErrorV1> {
        for entry in
            fs::read_dir(self.root.as_ref()).map_err(|_| HouseAllowanceErrorV1::Unavailable)?
        {
            let entry = entry.map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".allowances-") && name.ends_with(".tmp") {
                if !entry
                    .file_type()
                    .map_err(|_| HouseAllowanceErrorV1::Unavailable)?
                    .is_file()
                {
                    return Err(HouseAllowanceErrorV1::Corrupt);
                }
                validate_owner_only_file(&entry.path())
                    .map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
                fs::remove_file(entry.path()).map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
            }
        }
        sync_directory(self.root.as_ref()).map_err(|_| HouseAllowanceErrorV1::Unavailable)
    }

    fn load_or_empty(
        &self,
        period: HouseAllowancePeriodV1,
    ) -> Result<LedgerRecordV1, HouseAllowanceErrorV1> {
        if self.state_path.exists() {
            self.load()
        } else {
            Ok(LedgerRecordV1 {
                schema: LEDGER_SCHEMA_V1.to_owned(),
                day_index: period.day_index,
                month_index: period.month_index,
                daily_reserved_nano_usd: 0,
                monthly_reserved_nano_usd: 0,
                assignments: BTreeMap::new(),
                integrity_hash: String::new(),
            })
        }
    }

    fn load(&self) -> Result<LedgerRecordV1, HouseAllowanceErrorV1> {
        validate_owner_only_file(self.state_path.as_ref())
            .map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
        let metadata = fs::metadata(self.state_path.as_ref())
            .map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
        if metadata.len() > MAX_LEDGER_BYTES {
            return Err(HouseAllowanceErrorV1::Corrupt);
        }
        let bytes =
            fs::read(self.state_path.as_ref()).map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
        let record: LedgerRecordV1 =
            serde_json::from_slice(&bytes).map_err(|_| HouseAllowanceErrorV1::Corrupt)?;
        validate_ledger(&record, self.limits)?;
        Ok(record)
    }

    fn persist(&self, record: &mut LedgerRecordV1) -> Result<(), HouseAllowanceErrorV1> {
        record.integrity_hash = ledger_integrity_hash(record)?;
        validate_ledger(record, self.limits)?;
        let bytes = canonical_bytes(record).map_err(|()| HouseAllowanceErrorV1::Corrupt)?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_LEDGER_BYTES {
            return Err(HouseAllowanceErrorV1::AssignmentCapacity);
        }
        let temporary = self.root.join(format!(
            ".allowances-{}.tmp",
            random_hex().map_err(|()| HouseAllowanceErrorV1::Unavailable)?
        ));
        let mut file = create_owner_only_renameable_file(&temporary)
            .map_err(|_| HouseAllowanceErrorV1::Unavailable)?;
        let result = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| replace_file(&temporary, self.state_path.as_ref()))
            .and_then(|()| sync_directory(self.root.as_ref()))
            .map_err(|_| HouseAllowanceErrorV1::Unavailable);
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

fn roll_period(
    record: &mut LedgerRecordV1,
    period: HouseAllowancePeriodV1,
) -> Result<bool, HouseAllowanceErrorV1> {
    if period.day_index < record.day_index || period.month_index < record.month_index {
        return Err(HouseAllowanceErrorV1::Corrupt);
    }
    let mut changed = false;
    if period.month_index > record.month_index {
        record.month_index = period.month_index;
        record.monthly_reserved_nano_usd = 0;
        changed = true;
    }
    if period.day_index > record.day_index {
        record.day_index = period.day_index;
        record.daily_reserved_nano_usd = 0;
        changed = true;
    }
    Ok(changed)
}

fn validate_assignment_for_revision(
    assignment: &AssignmentRecordV1,
    revision: &HouseAgentRevision,
) -> Result<(), HouseAllowanceErrorV1> {
    if assignment.revision_digest == revision.digest() {
        Ok(())
    } else {
        Err(HouseAllowanceErrorV1::InvalidInput)
    }
}

fn validate_ledger(
    record: &LedgerRecordV1,
    limits: HouseSpendLimitsV1,
) -> Result<(), HouseAllowanceErrorV1> {
    if record.schema != LEDGER_SCHEMA_V1
        || record.assignments.len() > MAX_ASSIGNMENTS
        || record.daily_reserved_nano_usd > limits.daily_nano_usd
        || record.monthly_reserved_nano_usd > limits.monthly_nano_usd
        || record.daily_reserved_nano_usd > record.monthly_reserved_nano_usd
        || ledger_integrity_hash(record).as_deref() != Ok(record.integrity_hash.as_str())
    {
        return Err(HouseAllowanceErrorV1::Corrupt);
    }
    for (assignment_id, assignment) in &record.assignments {
        if !bounded_identity(assignment_id)
            || !valid_blake3_digest(&assignment.revision_digest)
            || assignment.attempts.len() > MAX_ATTEMPTS_PER_ASSIGNMENT
        {
            return Err(HouseAllowanceErrorV1::Corrupt);
        }
        let mut input = 0_u64;
        let mut output = 0_u64;
        for (attempt_digest, attempt) in &assignment.attempts {
            if !valid_blake3_digest(attempt_digest)
                || attempt.input_units == 0
                || attempt.maximum_output_units == 0
                || attempt.output_units > attempt.maximum_output_units
                || attempt.reserved_nano_usd == 0
                || attempt
                    .response_digest
                    .as_ref()
                    .is_some_and(|digest| !valid_blake3_digest(digest))
                || (attempt.state == AttemptStateV1::Completed && attempt.response_digest.is_none())
                || (attempt.state != AttemptStateV1::Completed
                    && (attempt.response_digest.is_some()
                        || attempt.provider_prompt_tokens.is_some()
                        || attempt.provider_completion_tokens.is_some()
                        || attempt.provider_cost_nano_usd.is_some()))
            {
                return Err(HouseAllowanceErrorV1::Corrupt);
            }
            input = input
                .checked_add(attempt.input_units)
                .ok_or(HouseAllowanceErrorV1::Corrupt)?;
            output = output
                .checked_add(attempt.output_units)
                .ok_or(HouseAllowanceErrorV1::Corrupt)?;
        }
        if input != assignment.consumed_input_units || output != assignment.consumed_output_units {
            return Err(HouseAllowanceErrorV1::Corrupt);
        }
    }
    Ok(())
}

fn ledger_integrity_hash(record: &LedgerRecordV1) -> Result<String, HouseAllowanceErrorV1> {
    let witness = LedgerIntegrityWitnessV1 {
        schema: &record.schema,
        day_index: record.day_index,
        month_index: record.month_index,
        daily_reserved_nano_usd: record.daily_reserved_nano_usd,
        monthly_reserved_nano_usd: record.monthly_reserved_nano_usd,
        assignments: &record.assignments,
    };
    let bytes = canonical_bytes(&witness).map_err(|()| HouseAllowanceErrorV1::Corrupt)?;
    Ok(blake3_digest(&bytes))
}

fn checked_cost(
    input_units: u64,
    prompt_nano_usd: u64,
    output_units: u64,
    completion_nano_usd: u64,
) -> Result<u64, HouseAllowanceErrorV1> {
    input_units
        .checked_mul(prompt_nano_usd)
        .and_then(|input| {
            output_units
                .checked_mul(completion_nano_usd)
                .and_then(|output| input.checked_add(output))
        })
        .ok_or(HouseAllowanceErrorV1::InvalidInput)
}

fn decimal_usd_to_nano_usd(value: &str) -> Result<u64, HouseAllowanceErrorV1> {
    let fraction = value
        .strip_prefix("0.")
        .ok_or(HouseAllowanceErrorV1::InvalidInput)?;
    if fraction.is_empty()
        || fraction.len() > 18
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(HouseAllowanceErrorV1::InvalidInput);
    }
    let leading = &fraction[..fraction.len().min(9)];
    let mut nanos = leading
        .parse::<u64>()
        .map_err(|_| HouseAllowanceErrorV1::InvalidInput)?;
    for _ in leading.len()..9 {
        nanos = nanos
            .checked_mul(10)
            .ok_or(HouseAllowanceErrorV1::InvalidInput)?;
    }
    if fraction
        .as_bytes()
        .get(9..)
        .is_some_and(|tail| tail.iter().any(|byte| *byte != b'0'))
    {
        nanos = nanos
            .checked_add(1)
            .ok_or(HouseAllowanceErrorV1::InvalidInput)?;
    }
    if nanos == 0 {
        return Err(HouseAllowanceErrorV1::InvalidInput);
    }
    Ok(nanos)
}

fn provider_max_price(value: &str) -> Result<Number, HouseModelErrorV1> {
    let fraction = value
        .strip_prefix("0.")
        .ok_or(HouseModelErrorV1::InvalidConfiguration)?;
    if fraction.is_empty()
        || fraction.len() > 18
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(HouseModelErrorV1::InvalidConfiguration);
    }
    let significant = fraction.trim_start_matches('0');
    let shifted = if significant.is_empty() {
        return Err(HouseModelErrorV1::InvalidConfiguration);
    } else if fraction.len() <= OPENROUTER_PRICE_SCALE {
        let zeros = "0".repeat(OPENROUTER_PRICE_SCALE - fraction.len());
        let integer = format!("{fraction}{zeros}");
        integer.trim_start_matches('0').to_owned()
    } else {
        let split = OPENROUTER_PRICE_SCALE;
        let integer = &fraction[..split];
        let decimal = fraction[split..].trim_end_matches('0');
        let integer = integer.trim_start_matches('0');
        let integer = if integer.is_empty() { "0" } else { integer };
        if decimal.is_empty() {
            integer.to_owned()
        } else {
            format!("{integer}.{decimal}")
        }
    };
    Number::from_str(&shifted).map_err(|_| HouseModelErrorV1::InvalidConfiguration)
}

fn canonical_bytes(value: &impl Serialize) -> Result<Vec<u8>, ()> {
    let encoded = serde_json::to_vec(value).map_err(|_| ())?;
    CanonicalJsonV1::parse(&encoded)
        .and_then(|canonical| canonical.to_bytes())
        .map_err(|_| ())
}

fn blake3_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn hash_field(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(bytes);
}

fn bounded_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_IDENTITY_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !matches!(byte, b'/' | b'\\'))
}

fn valid_blake3_digest(value: &str) -> bool {
    value.len() == 71
        && value
            .strip_prefix("blake3:")
            .is_some_and(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn random_hex() -> Result<String, ()> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| ())?;
    let mut result = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut result, "{byte:02x}").map_err(|_| ())?;
    }
    Ok(result)
}

fn map_poison<T>(_: PoisonError<T>) -> HouseAllowanceErrorV1 {
    HouseAllowanceErrorV1::Unavailable
}

#[cfg(unix)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let source = source.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source path is not Unicode",
        )
    })?;
    let target = target.to_str().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path is not Unicode",
        )
    })?;
    winsafe::MoveFileEx(
        source,
        Some(target),
        winsafe::co::MOVEFILE::REPLACE_EXISTING,
    )
    .map_err(|error| std::io::Error::other(error.to_string()))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt as _};

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

/// A provider credential that is never printable, serializable, cloneable, or persisted.
pub struct HouseProviderCredentialV1(Zeroizing<Vec<u8>>);

impl HouseProviderCredentialV1 {
    /// Takes ownership of a private credential buffer.
    ///
    /// # Errors
    ///
    /// Rejects values that could escape an HTTP header or exceed the private frame bound.
    pub fn new(value: Zeroizing<Vec<u8>>) -> Result<Self, HouseModelErrorV1> {
        if !(32..=512).contains(&value.len())
            || !value
                .iter()
                .all(|byte| byte.is_ascii_graphic() && !matches!(*byte, b':' | b'\\'))
        {
            return Err(HouseModelErrorV1::InvalidConfiguration);
        }
        Ok(Self(value))
    }

    fn expose(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// Closed network/provider failures with no upstream body or credential data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HouseProviderPortErrorV1 {
    #[error("House provider connection is unavailable")]
    Unavailable,
    #[error("House provider call timed out")]
    Timeout,
    #[error("House provider reply was lost or incomplete")]
    LostReply,
    #[error("House provider rate limit rejected the call")]
    RateLimited,
    #[error("House provider rejected the call")]
    Rejected,
}

/// Opaque exact request delivered to one replaceable provider adapter.
pub struct HouseProviderRequestV1 {
    body: Zeroizing<Vec<u8>>,
    timeout: Duration,
    model_slug: String,
    provider_slug: String,
}

impl HouseProviderRequestV1 {
    /// Returns the short-lived canonical request body to a provider adapter.
    #[must_use]
    pub fn body(&self) -> &[u8] {
        self.body.as_slice()
    }

    /// Returns the exact call timeout frozen by the House Agent Revision.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Returns the exact canonical model slug.
    #[must_use]
    pub fn model_slug(&self) -> &str {
        &self.model_slug
    }

    /// Returns the exact full provider slug.
    #[must_use]
    pub fn provider_slug(&self) -> &str {
        &self.provider_slug
    }
}

/// Opaque bounded provider reply. Its body is zeroized and has no `Debug` implementation.
pub struct HouseProviderReplyV1 {
    body: Zeroizing<Vec<u8>>,
}

impl HouseProviderReplyV1 {
    /// Constructs a bounded reply for a replaceable provider adapter.
    ///
    /// # Errors
    ///
    /// Rejects an empty or oversized provider body.
    pub fn new(body: Vec<u8>) -> Result<Self, HouseProviderPortErrorV1> {
        if body.is_empty() || body.len() > MAX_PROVIDER_RESPONSE_BYTES {
            return Err(HouseProviderPortErrorV1::LostReply);
        }
        Ok(Self {
            body: Zeroizing::new(body),
        })
    }

    fn body(&self) -> &[u8] {
        self.body.as_slice()
    }
}

/// Replaceable synchronous provider boundary used by the authority-free model host.
pub trait HouseProviderPortV1: Send + Sync + 'static {
    /// Dispatches one exact, already-bounded request without retries or redirects.
    ///
    /// # Errors
    ///
    /// Returns only a closed transport/provider failure class.
    fn dispatch(
        &self,
        request: &HouseProviderRequestV1,
        credential: &HouseProviderCredentialV1,
    ) -> Result<HouseProviderReplyV1, HouseProviderPortErrorV1>;
}

impl<T: HouseProviderPortV1> HouseProviderPortV1 for Arc<T> {
    fn dispatch(
        &self,
        request: &HouseProviderRequestV1,
        credential: &HouseProviderCredentialV1,
    ) -> Result<HouseProviderReplyV1, HouseProviderPortErrorV1> {
        self.as_ref().dispatch(request, credential)
    }
}

/// Production `OpenRouter` HTTPS adapter with a compiled origin and path.
#[derive(Clone)]
pub struct OpenRouterProviderPortV1 {
    tls: Arc<ClientConfig>,
}

impl std::fmt::Debug for OpenRouterProviderPortV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("OpenRouterProviderPortV1(verified, fixed-origin)")
    }
}

impl Default for OpenRouterProviderPortV1 {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenRouterProviderPortV1 {
    /// Builds a verified rustls client from the Mozilla `WebPKI` root bundle.
    #[must_use]
    pub fn new() -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Self { tls: Arc::new(tls) }
    }
}

impl HouseProviderPortV1 for OpenRouterProviderPortV1 {
    fn dispatch(
        &self,
        request: &HouseProviderRequestV1,
        credential: &HouseProviderCredentialV1,
    ) -> Result<HouseProviderReplyV1, HouseProviderPortErrorV1> {
        let tcp = connect_openrouter(request.timeout)?;
        tcp.set_read_timeout(Some(request.timeout))
            .and_then(|()| tcp.set_write_timeout(Some(request.timeout)))
            .map_err(|_| HouseProviderPortErrorV1::Unavailable)?;
        let server_name = ServerName::try_from(OPENROUTER_AUTHORITY.to_owned())
            .map_err(|_| HouseProviderPortErrorV1::Unavailable)?;
        let connection = ClientConnection::new(Arc::clone(&self.tls), server_name)
            .map_err(|_| HouseProviderPortErrorV1::Unavailable)?;
        let mut stream = StreamOwned::new(connection, tcp);
        write_openrouter_request(&mut stream, request, credential)?;
        stream.flush().map_err(map_write_error)?;
        // Content-Length frames the request. A raw TCP half-close here
        // truncates the TLS conversation: OpenRouter closes without a reply.
        // Keep both directions open until the HTTP response has been read.
        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_PROVIDER_RESPONSE_BYTES + 8_192).unwrap_or(u64::MAX))
            .read_to_end(&mut response)
            .map_err(map_read_error)?;
        parse_openrouter_http_response(&response)
    }
}

/// Development-only plain-HTTP adapter for the loopback fake `OpenRouter`.
///
/// Construction rejects every non-loopback or zero-port address. Production
/// code must continue to use [`OpenRouterProviderPortV1`], whose origin and TLS
/// policy are compiled into the binary.
#[derive(Clone, Copy, Debug)]
pub struct DevelopmentLoopbackOpenRouterProviderPortV1 {
    address: SocketAddr,
}

impl DevelopmentLoopbackOpenRouterProviderPortV1 {
    /// Selects one exact loopback endpoint for a visible local substitute.
    ///
    /// # Errors
    ///
    /// Rejects a non-loopback address or port zero.
    pub fn new(address: SocketAddr) -> Result<Self, HouseProviderPortErrorV1> {
        if !address.ip().is_loopback() || address.port() == 0 {
            return Err(HouseProviderPortErrorV1::Rejected);
        }
        Ok(Self { address })
    }
}

impl HouseProviderPortV1 for DevelopmentLoopbackOpenRouterProviderPortV1 {
    fn dispatch(
        &self,
        request: &HouseProviderRequestV1,
        credential: &HouseProviderCredentialV1,
    ) -> Result<HouseProviderReplyV1, HouseProviderPortErrorV1> {
        let mut stream = TcpStream::connect_timeout(&self.address, request.timeout)
            .map_err(|_| HouseProviderPortErrorV1::Unavailable)?;
        stream
            .set_read_timeout(Some(request.timeout))
            .and_then(|()| stream.set_write_timeout(Some(request.timeout)))
            .map_err(|_| HouseProviderPortErrorV1::Unavailable)?;
        write!(
            stream,
            "POST {OPENROUTER_PATH} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer ",
            self.address
        )
        .map_err(map_write_error)?;
        stream
            .write_all(credential.expose())
            .map_err(map_write_error)?;
        write!(
            stream,
            "\r\nContent-Type: application/json\r\nAccept: application/json\r\nX-OpenRouter-Metadata: enabled\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            request.body().len()
        )
        .map_err(map_write_error)?;
        stream.write_all(request.body()).map_err(map_write_error)?;
        stream.flush().map_err(map_write_error)?;
        stream.shutdown(Shutdown::Write).map_err(map_write_error)?;
        let mut response = Vec::new();
        stream
            .take(u64::try_from(MAX_PROVIDER_RESPONSE_BYTES + 8_192).unwrap_or(u64::MAX))
            .read_to_end(&mut response)
            .map_err(map_read_error)?;
        parse_openrouter_http_response(&response)
    }
}

fn connect_openrouter(timeout: Duration) -> Result<TcpStream, HouseProviderPortErrorV1> {
    let addresses = (OPENROUTER_AUTHORITY, OPENROUTER_PORT)
        .to_socket_addrs()
        .map_err(|_| HouseProviderPortErrorV1::Unavailable)?;
    let deadline = Instant::now() + timeout;
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HouseProviderPortErrorV1::Timeout);
        }
        if let Ok(stream) = TcpStream::connect_timeout(&address, remaining) {
            return Ok(stream);
        }
    }
    Err(HouseProviderPortErrorV1::Unavailable)
}

fn write_openrouter_request(
    stream: &mut impl std::io::Write,
    request: &HouseProviderRequestV1,
    credential: &HouseProviderCredentialV1,
) -> Result<(), HouseProviderPortErrorV1> {
    write!(
        stream,
        "POST {OPENROUTER_PATH} HTTP/1.1\r\nHost: {OPENROUTER_AUTHORITY}\r\nAuthorization: Bearer "
    )
    .map_err(map_write_error)?;
    stream
        .write_all(credential.expose())
        .map_err(map_write_error)?;
    write!(
        stream,
        "\r\nContent-Type: application/json\r\nAccept: application/json\r\nX-OpenRouter-Metadata: enabled\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        request.body().len()
    )
    .map_err(map_write_error)?;
    stream.write_all(request.body()).map_err(map_write_error)
}

fn parse_openrouter_http_response(
    response: &[u8],
) -> Result<HouseProviderReplyV1, HouseProviderPortErrorV1> {
    let header_end =
        find_bytes(response, b"\r\n\r\n").ok_or(HouseProviderPortErrorV1::LostReply)?;
    if header_end > 8_192 {
        return Err(HouseProviderPortErrorV1::LostReply);
    }
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|_| HouseProviderPortErrorV1::LostReply)?;
    let mut lines = headers.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or(HouseProviderPortErrorV1::LostReply)?;
    let mut content_length = None;
    let mut chunked = false;
    let mut json_content = false;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .ok_or(HouseProviderPortErrorV1::LostReply)?;
        let name = name.trim();
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(HouseProviderPortErrorV1::LostReply);
            }
            content_length = Some(
                value
                    .parse::<usize>()
                    .map_err(|_| HouseProviderPortErrorV1::LostReply)?,
            );
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if !value.eq_ignore_ascii_case("chunked") {
                return Err(HouseProviderPortErrorV1::LostReply);
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case("content-type") {
            json_content = value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/json"));
        } else if name.eq_ignore_ascii_case("content-encoding")
            && !value.eq_ignore_ascii_case("identity")
        {
            return Err(HouseProviderPortErrorV1::LostReply);
        }
    }
    if !json_content || (chunked && content_length.is_some()) {
        return Err(HouseProviderPortErrorV1::LostReply);
    }
    let encoded_body = response
        .get(header_end + 4..)
        .ok_or(HouseProviderPortErrorV1::LostReply)?;
    let body = if chunked {
        decode_chunked(encoded_body)?
    } else if let Some(length) = content_length {
        if length != encoded_body.len() {
            return Err(HouseProviderPortErrorV1::LostReply);
        }
        encoded_body.to_vec()
    } else {
        encoded_body.to_vec()
    };
    if body.is_empty() || body.len() > MAX_PROVIDER_RESPONSE_BYTES {
        return Err(HouseProviderPortErrorV1::LostReply);
    }
    match status {
        200..=299 => HouseProviderReplyV1::new(body),
        429 => Err(HouseProviderPortErrorV1::RateLimited),
        500..=599 => Err(HouseProviderPortErrorV1::LostReply),
        _ => Err(HouseProviderPortErrorV1::Rejected),
    }
}

fn decode_chunked(encoded: &[u8]) -> Result<Vec<u8>, HouseProviderPortErrorV1> {
    let mut cursor = 0_usize;
    let mut decoded = Vec::new();
    loop {
        let relative_end = find_bytes(
            encoded
                .get(cursor..)
                .ok_or(HouseProviderPortErrorV1::LostReply)?,
            b"\r\n",
        )
        .ok_or(HouseProviderPortErrorV1::LostReply)?;
        let line_end = cursor
            .checked_add(relative_end)
            .ok_or(HouseProviderPortErrorV1::LostReply)?;
        let size_text = std::str::from_utf8(
            encoded
                .get(cursor..line_end)
                .ok_or(HouseProviderPortErrorV1::LostReply)?,
        )
        .map_err(|_| HouseProviderPortErrorV1::LostReply)?;
        let size_text = size_text
            .split_once(';')
            .map_or(size_text, |(size, _)| size);
        let size = usize::from_str_radix(size_text.trim(), 16)
            .map_err(|_| HouseProviderPortErrorV1::LostReply)?;
        cursor = line_end
            .checked_add(2)
            .ok_or(HouseProviderPortErrorV1::LostReply)?;
        if size == 0 {
            return Ok(decoded);
        }
        let chunk_end = cursor
            .checked_add(size)
            .ok_or(HouseProviderPortErrorV1::LostReply)?;
        if chunk_end
            .checked_add(2)
            .is_none_or(|end| end > encoded.len())
            || encoded.get(chunk_end..chunk_end + 2) != Some(b"\r\n")
        {
            return Err(HouseProviderPortErrorV1::LostReply);
        }
        decoded.extend_from_slice(
            encoded
                .get(cursor..chunk_end)
                .ok_or(HouseProviderPortErrorV1::LostReply)?,
        );
        if decoded.len() > MAX_PROVIDER_RESPONSE_BYTES {
            return Err(HouseProviderPortErrorV1::LostReply);
        }
        cursor = chunk_end + 2;
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn map_write_error(error: std::io::Error) -> HouseProviderPortErrorV1 {
    let kind = error.kind();
    drop(error);
    if matches!(
        kind,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        HouseProviderPortErrorV1::Timeout
    } else {
        HouseProviderPortErrorV1::LostReply
    }
}

fn map_read_error(error: std::io::Error) -> HouseProviderPortErrorV1 {
    map_write_error(error)
}

/// One schema-valid Action proposal. It contains no model reasoning or raw reply.
#[derive(Clone, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HouseProposedActionV1 {
    pub offer_id: String,
    pub payload: Value,
}

/// Safe provider evidence retained after a validated response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HouseProviderEvidenceV1 {
    pub generation_id: String,
    pub request_digest: String,
    pub response_digest: String,
    pub provider_prompt_tokens: u64,
    pub provider_completion_tokens: u64,
    pub provider_cost_nano_usd: Option<u64>,
}

/// Validated result returned to the authority-holding assignment helper.
pub struct HouseModelCompletionV1 {
    pub action: HouseProposedActionV1,
    pub evidence: HouseProviderEvidenceV1,
}

/// Closed model-execution failures safe for ordinary operational logs.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum HouseModelErrorV1 {
    #[error("House model configuration is invalid")]
    InvalidConfiguration,
    #[error("House model input is invalid")]
    InvalidInput,
    #[error("House model input exceeds its hard accounting limit")]
    InputLimit,
    #[error("House model output exceeds its hard accounting limit")]
    OutputLimit,
    #[error("House model output does not match the required schema")]
    InvalidResponse,
    #[error("House model output selected an unavailable Action Offer")]
    UnofferedAction,
    #[error("House provider route evidence does not match the immutable route")]
    RouteMismatch,
    #[error("House provider fallback or request transformation was observed")]
    FallbackDetected,
    #[error(transparent)]
    Allowance(#[from] HouseAllowanceErrorV1),
    #[error(transparent)]
    Provider(#[from] HouseProviderPortErrorV1),
}

/// Combines one replaceable provider port with the durable at-most-once ledger.
pub struct HouseModelExecutorV1<P> {
    provider: P,
    ledger: FileHouseAllowanceLedgerV1,
}

impl<P: HouseProviderPortV1> HouseModelExecutorV1<P> {
    #[must_use]
    pub const fn new(provider: P, ledger: FileHouseAllowanceLedgerV1) -> Self {
        Self { provider, ledger }
    }

    /// Executes one bounded House Invocation at most once.
    ///
    /// # Errors
    ///
    /// Fails closed before dispatch for bad/oversized input and consumes the
    /// durable reservation for every failure after provider dispatch may begin.
    pub fn execute(
        &self,
        credential: &HouseProviderCredentialV1,
        revision: &HouseAgentRevision,
        identity: &HouseInvocationIdentityV1,
        projection: &Value,
        action_offers: &Value,
        period: HouseAllowancePeriodV1,
    ) -> Result<HouseModelCompletionV1, HouseModelErrorV1> {
        let (request, offered_schemas) =
            build_provider_request(revision, projection, action_offers)?;
        let input_units = u64::try_from(request.body().len()).unwrap_or(u64::MAX);
        if input_units > revision.allowance().input_tokens_per_call {
            return Err(HouseModelErrorV1::InputLimit);
        }
        let request_digest = blake3_digest(request.body());
        let reservation = self
            .ledger
            .reserve(identity, revision, input_units, period)?;
        let reply = match self.provider.dispatch(&request, credential) {
            Ok(reply) => reply,
            Err(error) => {
                let state = match error {
                    HouseProviderPortErrorV1::RateLimited | HouseProviderPortErrorV1::Rejected => {
                        AttemptStateV1::ProviderFailed
                    }
                    HouseProviderPortErrorV1::Unavailable
                    | HouseProviderPortErrorV1::Timeout
                    | HouseProviderPortErrorV1::LostReply => AttemptStateV1::Ambiguous,
                };
                self.ledger.consume_failed(&reservation, state)?;
                return Err(HouseModelErrorV1::Provider(error));
            }
        };
        let decoded = match decode_provider_reply(
            &reply,
            revision,
            &offered_schemas,
            request_digest.as_str(),
        ) {
            Ok(decoded) => decoded,
            Err(error) => {
                self.ledger
                    .consume_failed(&reservation, AttemptStateV1::ProviderFailed)?;
                return Err(error);
            }
        };
        let output_units = u64::try_from(decoded.canonical_action_bytes).unwrap_or(u64::MAX);
        self.ledger
            .complete(&reservation, output_units, &decoded.completion.evidence)?;
        Ok(decoded.completion)
    }
}

fn build_provider_request(
    revision: &HouseAgentRevision,
    projection: &Value,
    action_offers: &Value,
) -> Result<(HouseProviderRequestV1, BTreeMap<String, Value>), HouseModelErrorV1> {
    if revision.accounting_tokenizer() != (ACCOUNTING_TOKENIZER_ID, ACCOUNTING_TOKENIZER_REVISION) {
        return Err(HouseModelErrorV1::InvalidConfiguration);
    }
    let offered_schemas = exact_offer_schemas(action_offers)?;
    let invocation = json!({
        "action_offers": action_offers,
        "instruction": "Return exactly one JSON object with offer_id and payload. Select only a listed offer. Do not include prose or reasoning.",
        "projection": projection,
        "schema": "worldstream/house-model-invocation/v1"
    });
    let invocation_bytes =
        canonical_bytes(&invocation).map_err(|()| HouseModelErrorV1::InvalidInput)?;
    let invocation_text =
        String::from_utf8(invocation_bytes).map_err(|_| HouseModelErrorV1::InvalidInput)?;
    let (_, _, behavior_instructions) = revision.behavior_policy();
    let provider = json!({
        "allow_fallbacks": false,
        "data_collection": "deny",
        "max_price": {
            "completion": provider_max_price(revision.maximum_completion_price())?,
            "prompt": provider_max_price(revision.maximum_prompt_price())?
        },
        "only": [revision.provider_slug()],
        "order": [revision.provider_slug()],
        "require_parameters": true,
        "zdr": true
    });
    let mut body = serde_json::Map::new();
    body.insert(
        "model".to_owned(),
        Value::String(revision.model_slug().to_owned()),
    );
    body.insert(
        "messages".to_owned(),
        json!([
            {"role": "system", "content": behavior_instructions},
            {"role": "user", "content": invocation_text}
        ]),
    );
    body.insert("provider".to_owned(), provider);
    body.insert("response_format".to_owned(), json!({"type": "json_object"}));
    body.insert("stream".to_owned(), Value::Bool(false));
    let output_limit = Value::Number(Number::from(revision.allowance().output_tokens_per_call));
    match revision.completion_token_parameter() {
        HouseAgentCompletionTokenParameterV1::MaxTokens => {
            body.insert("max_tokens".to_owned(), output_limit);
        }
        HouseAgentCompletionTokenParameterV1::MaxCompletionTokens => {
            body.insert("max_completion_tokens".to_owned(), output_limit);
        }
    }
    // OpenRouter price ceilings are JSON decimals, which are intentionally not
    // part of WorldStream's integer-only Canonical JSON domain. `serde_json`'s
    // sorted map representation still gives this closed request one stable byte
    // encoding for local accounting and request hashing.
    let body =
        serde_json::to_vec(&Value::Object(body)).map_err(|_| HouseModelErrorV1::InvalidInput)?;
    Ok((
        HouseProviderRequestV1 {
            body: Zeroizing::new(body),
            timeout: Duration::from_secs(revision.allowance().call_timeout_seconds),
            model_slug: revision.model_slug().to_owned(),
            provider_slug: revision.provider_slug().to_owned(),
        },
        offered_schemas,
    ))
}

fn exact_offer_schemas(
    action_offers: &Value,
) -> Result<BTreeMap<String, Value>, HouseModelErrorV1> {
    let object = action_offers
        .as_object()
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    if object.get("schema").and_then(Value::as_str)
        != Some("worldstream/assignment-action-offer-list/v1")
    {
        return Err(HouseModelErrorV1::InvalidInput);
    }
    let offers = object
        .get("offers")
        .and_then(Value::as_array)
        .ok_or(HouseModelErrorV1::InvalidInput)?;
    if offers.is_empty() || offers.len() > 32 {
        return Err(HouseModelErrorV1::InvalidInput);
    }
    let mut schemas = BTreeMap::new();
    for offer in offers {
        let offer = offer.as_object().ok_or(HouseModelErrorV1::InvalidInput)?;
        let id = offer
            .get("offer_id")
            .and_then(Value::as_str)
            .filter(|id| bounded_identity(id))
            .ok_or(HouseModelErrorV1::InvalidInput)?;
        let schema = offer
            .get("payload_schema")
            .and_then(Value::as_object)
            .and_then(|reference| reference.get("schema"))
            .filter(|schema| action_payload_schema_is_valid_v1(schema))
            .ok_or(HouseModelErrorV1::InvalidInput)?;
        if schemas.insert(id.to_owned(), schema.clone()).is_some() {
            return Err(HouseModelErrorV1::InvalidInput);
        }
    }
    Ok(schemas)
}

#[derive(Deserialize)]
struct ProviderResponseWireV1 {
    id: String,
    object: String,
    model: String,
    choices: Vec<ProviderChoiceWireV1>,
    usage: ProviderUsageWireV1,
    openrouter_metadata: RouterMetadataWireV1,
}

#[derive(Deserialize)]
struct ProviderChoiceWireV1 {
    index: u64,
    finish_reason: String,
    message: ProviderMessageWireV1,
}

#[derive(Deserialize)]
struct ProviderMessageWireV1 {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ProviderUsageWireV1 {
    prompt_tokens: u64,
    completion_tokens: u64,
    total_tokens: u64,
    cost: Option<Number>,
}

#[derive(Deserialize)]
struct RouterMetadataWireV1 {
    requested: String,
    strategy: String,
    attempt: u64,
    endpoints: RouterEndpointsWireV1,
    #[serde(default, deserialize_with = "deserialize_present_attempt_history")]
    attempts: Option<Vec<RouterAttemptWireV1>>,
    pipeline: Option<Vec<Value>>,
}

fn deserialize_present_attempt_history<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<RouterAttemptWireV1>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // OpenRouter permits omission, but its schema does not permit explicit null.
    Vec::<RouterAttemptWireV1>::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct RouterEndpointsWireV1 {
    total: u64,
    available: Vec<RouterEndpointWireV1>,
}

#[derive(Deserialize)]
struct RouterEndpointWireV1 {
    provider: String,
    model: String,
    selected: bool,
}

#[derive(Deserialize)]
struct RouterAttemptWireV1 {
    provider: String,
    model: String,
    status: u16,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProposedActionWireV1 {
    offer_id: String,
    payload: Value,
}

struct DecodedProviderReplyV1 {
    completion: HouseModelCompletionV1,
    canonical_action_bytes: usize,
}

fn decode_provider_reply(
    reply: &HouseProviderReplyV1,
    revision: &HouseAgentRevision,
    offered_schemas: &BTreeMap<String, Value>,
    request_digest: &str,
) -> Result<DecodedProviderReplyV1, HouseModelErrorV1> {
    let response: ProviderResponseWireV1 =
        serde_json::from_slice(reply.body()).map_err(|_| HouseModelErrorV1::InvalidResponse)?;
    if response.id.len() > MAX_GENERATION_ID_BYTES
        || !response.id.starts_with("gen-")
        || response.object != "chat.completion"
        || response.choices.len() != 1
    {
        return Err(HouseModelErrorV1::InvalidResponse);
    }
    if !response_model_corresponds(&response.model, revision.model_slug())
        || response.openrouter_metadata.requested != revision.model_slug()
    {
        return Err(HouseModelErrorV1::RouteMismatch);
    }
    validate_route_metadata(&response.openrouter_metadata, revision)?;
    let choice = response
        .choices
        .first()
        .ok_or(HouseModelErrorV1::InvalidResponse)?;
    if choice.index != 0 || choice.finish_reason != "stop" || choice.message.role != "assistant" {
        return Err(HouseModelErrorV1::InvalidResponse);
    }
    if choice.message.content.len()
        > usize::try_from(revision.allowance().output_tokens_per_call).unwrap_or(usize::MAX)
    {
        return Err(HouseModelErrorV1::OutputLimit);
    }
    let action_canonical = CanonicalJsonV1::parse(choice.message.content.as_bytes())
        .and_then(|value| value.to_bytes())
        .map_err(|_| HouseModelErrorV1::InvalidResponse)?;
    let action: ProposedActionWireV1 = serde_json::from_slice(&action_canonical)
        .map_err(|_| HouseModelErrorV1::InvalidResponse)?;
    let payload_schema = offered_schemas
        .get(&action.offer_id)
        .ok_or(HouseModelErrorV1::UnofferedAction)?;
    if !action_payload_matches_schema_v1(payload_schema, &action.payload) {
        return Err(HouseModelErrorV1::InvalidResponse);
    }
    let usage_total = response
        .usage
        .prompt_tokens
        .checked_add(response.usage.completion_tokens)
        .ok_or(HouseModelErrorV1::InvalidResponse)?;
    if usage_total != response.usage.total_tokens
        || response.usage.completion_tokens > revision.allowance().output_tokens_per_call
    {
        return Err(HouseModelErrorV1::InvalidResponse);
    }
    let provider_cost_nano_usd = response
        .usage
        .cost
        .as_ref()
        .map(number_to_nano_usd)
        .transpose()?;
    let evidence = HouseProviderEvidenceV1 {
        generation_id: response.id,
        request_digest: request_digest.to_owned(),
        response_digest: blake3_digest(reply.body()),
        provider_prompt_tokens: response.usage.prompt_tokens,
        provider_completion_tokens: response.usage.completion_tokens,
        provider_cost_nano_usd,
    };
    Ok(DecodedProviderReplyV1 {
        completion: HouseModelCompletionV1 {
            action: HouseProposedActionV1 {
                offer_id: action.offer_id,
                payload: action.payload,
            },
            evidence,
        },
        canonical_action_bytes: action_canonical.len(),
    })
}

fn validate_route_metadata(
    metadata: &RouterMetadataWireV1,
    revision: &HouseAgentRevision,
) -> Result<(), HouseModelErrorV1> {
    if metadata.strategy != "direct"
        || metadata.attempt != 1
        || metadata
            .pipeline
            .as_ref()
            .is_some_and(|pipeline| !pipeline.is_empty())
    {
        return Err(HouseModelErrorV1::FallbackDetected);
    }
    // `total` counts catalog candidates before filtering, not attempted calls.
    // The selected endpoint and first-success attempt remain exact below.
    if metadata.endpoints.total == 0 || metadata.endpoints.available.len() != 1 {
        return Err(HouseModelErrorV1::RouteMismatch);
    }
    let endpoint = metadata
        .endpoints
        .available
        .first()
        .ok_or(HouseModelErrorV1::RouteMismatch)?;
    if !endpoint.selected
        || endpoint.model != revision.model_slug()
        || !provider_corresponds(&endpoint.provider, revision.provider_slug())
    {
        return Err(HouseModelErrorV1::RouteMismatch);
    }
    // The documented first-success counter and selected endpoint prove the
    // single attempt above. OpenRouter may omit its optional attempt history.
    let Some(attempts) = metadata.attempts.as_ref() else {
        return Ok(());
    };
    if attempts.len() != 1 {
        return Err(HouseModelErrorV1::FallbackDetected);
    }
    let attempt = attempts
        .first()
        .ok_or(HouseModelErrorV1::FallbackDetected)?;
    if attempt.status != 200
        || attempt.model != revision.model_slug()
        || !provider_corresponds(&attempt.provider, revision.provider_slug())
    {
        return Err(HouseModelErrorV1::RouteMismatch);
    }
    Ok(())
}

fn response_model_corresponds(reported: &str, configured: &str) -> bool {
    // OpenRouter's verified endpoint catalog maps this exact dated slug to
    // the canonical response ID. Do not generally strip model revisions.
    reported == configured
        || (configured == "ibm-granite/granite-4.2-8b-20260831"
            && reported == "ibm-granite/granite-4.2-8b")
}

fn provider_corresponds(reported: &str, configured: &str) -> bool {
    let reported = normalize_provider(reported);
    let configured_full = normalize_provider(configured);
    let configured_base = normalize_provider(configured.split('/').next().unwrap_or(configured));
    !reported.is_empty() && (reported == configured_full || reported == configured_base)
}

fn normalize_provider(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}

fn number_to_nano_usd(number: &Number) -> Result<u64, HouseModelErrorV1> {
    let text = number.to_string();
    if text.starts_with('-') {
        return Err(HouseModelErrorV1::InvalidResponse);
    }
    let (mantissa, exponent) = text
        .split_once(['e', 'E'])
        .map_or((text.as_str(), 0_i32), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i32>().unwrap_or(i32::MIN))
        });
    if exponent == i32::MIN || exponent.unsigned_abs() > 100 {
        return Err(HouseModelErrorV1::InvalidResponse);
    }
    let (integer, fraction) = mantissa
        .split_once('.')
        .map_or((mantissa, ""), |parts| parts);
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(HouseModelErrorV1::InvalidResponse);
    }
    let digits = format!("{integer}{fraction}")
        .parse::<u128>()
        .map_err(|_| HouseModelErrorV1::InvalidResponse)?;
    let fraction_len =
        i32::try_from(fraction.len()).map_err(|_| HouseModelErrorV1::InvalidResponse)?;
    let decimal_places = fraction_len
        .checked_sub(exponent)
        .ok_or(HouseModelErrorV1::InvalidResponse)?;
    let nanos = if decimal_places <= 9 {
        let power =
            u32::try_from(9 - decimal_places).map_err(|_| HouseModelErrorV1::InvalidResponse)?;
        digits
            .checked_mul(pow10(power)?)
            .ok_or(HouseModelErrorV1::InvalidResponse)?
    } else {
        let power =
            u32::try_from(decimal_places - 9).map_err(|_| HouseModelErrorV1::InvalidResponse)?;
        let divisor = pow10(power)?;
        digits
            .checked_add(divisor - 1)
            .and_then(|value| value.checked_div(divisor))
            .ok_or(HouseModelErrorV1::InvalidResponse)?
    };
    u64::try_from(nanos).map_err(|_| HouseModelErrorV1::InvalidResponse)
}

fn pow10(power: u32) -> Result<u128, HouseModelErrorV1> {
    10_u128
        .checked_pow(power)
        .ok_or(HouseModelErrorV1::InvalidResponse)
}

/// Deterministic fault modes for the local conformance provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeterministicHouseProviderFaultV1 {
    None,
    InvalidSchema,
    RouteMismatch,
    FallbackEvidence,
    OutputTooLarge,
    Timeout,
    LostReply,
    RateLimited,
    Rejected,
}

/// In-process deterministic provider for local development and conformance tests.
pub struct DeterministicHouseProviderPortV1 {
    action: HouseProposedActionV1,
    fault: DeterministicHouseProviderFaultV1,
    calls: std::sync::atomic::AtomicU64,
}

impl DeterministicHouseProviderPortV1 {
    #[must_use]
    pub const fn new(
        action: HouseProposedActionV1,
        fault: DeterministicHouseProviderFaultV1,
    ) -> Self {
        Self {
            action,
            fault,
            calls: std::sync::atomic::AtomicU64::new(0),
        }
    }

    #[must_use]
    pub fn call_count(&self) -> u64 {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl HouseProviderPortV1 for DeterministicHouseProviderPortV1 {
    fn dispatch(
        &self,
        request: &HouseProviderRequestV1,
        _credential: &HouseProviderCredentialV1,
    ) -> Result<HouseProviderReplyV1, HouseProviderPortErrorV1> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        match self.fault {
            DeterministicHouseProviderFaultV1::Timeout => {
                return Err(HouseProviderPortErrorV1::Timeout);
            }
            DeterministicHouseProviderFaultV1::LostReply => {
                return Err(HouseProviderPortErrorV1::LostReply);
            }
            DeterministicHouseProviderFaultV1::RateLimited => {
                return Err(HouseProviderPortErrorV1::RateLimited);
            }
            DeterministicHouseProviderFaultV1::Rejected => {
                return Err(HouseProviderPortErrorV1::Rejected);
            }
            DeterministicHouseProviderFaultV1::None
            | DeterministicHouseProviderFaultV1::InvalidSchema
            | DeterministicHouseProviderFaultV1::RouteMismatch
            | DeterministicHouseProviderFaultV1::FallbackEvidence
            | DeterministicHouseProviderFaultV1::OutputTooLarge => {}
        }
        if self.fault == DeterministicHouseProviderFaultV1::InvalidSchema {
            return HouseProviderReplyV1::new(br#"{"unexpected":true}"#.to_vec());
        }
        let action = if self.fault == DeterministicHouseProviderFaultV1::OutputTooLarge {
            "x".repeat(1_001)
        } else {
            let bytes =
                canonical_bytes(&self.action).map_err(|()| HouseProviderPortErrorV1::Rejected)?;
            String::from_utf8(bytes).map_err(|_| HouseProviderPortErrorV1::Rejected)?
        };
        let response_model = if self.fault == DeterministicHouseProviderFaultV1::RouteMismatch {
            "wrong/model"
        } else {
            request.model_slug()
        };
        let fallback = self.fault == DeterministicHouseProviderFaultV1::FallbackEvidence;
        let attempts = if fallback {
            json!([
                {"provider": request.provider_slug(), "model": request.model_slug(), "status": 503},
                {"provider": request.provider_slug(), "model": request.model_slug(), "status": 200}
            ])
        } else {
            json!([
                {"provider": request.provider_slug(), "model": request.model_slug(), "status": 200}
            ])
        };
        let body = serde_json::to_vec(&json!({
            "choices": [{
                "finish_reason": "stop",
                "index": 0,
                "message": {"content": action, "role": "assistant"}
            }],
            "id": "gen-worldstream-deterministic",
            "model": response_model,
            "object": "chat.completion",
            "openrouter_metadata": {
                "attempt": if fallback { 2 } else { 1 },
                "attempts": attempts,
                "endpoints": {
                    "available": [{
                        "model": request.model_slug(),
                        "provider": request.provider_slug(),
                        "selected": true
                    }],
                    "total": 1
                },
                "requested": request.model_slug(),
                "strategy": if fallback { "fallback" } else { "direct" }
            },
            "usage": {"completion_tokens": 1, "cost": 0, "prompt_tokens": 1, "total_tokens": 2}
        }))
        .map_err(|_| HouseProviderPortErrorV1::Rejected)?;
        HouseProviderReplyV1::new(body)
    }
}

#[cfg(test)]
mod tests {
    use std::{error::Error, fs, str::FromStr as _, sync::Arc};

    use serde_json::{Number, Value, json};
    use tempfile::tempdir;
    use worldstream_core::CanonicalJsonV1;
    use worldstream_hosted_contract::HouseAgentRevision;
    use zeroize::Zeroizing;

    use super::{
        AttemptStateV1, DeterministicHouseProviderFaultV1, DeterministicHouseProviderPortV1,
        DevelopmentLoopbackOpenRouterProviderPortV1, FileHouseAllowanceLedgerV1,
        HouseAllowanceErrorV1, HouseAllowancePeriodV1, HouseInvocationIdentityV1,
        HouseModelErrorV1, HouseModelExecutorV1, HouseProposedActionV1, HouseProviderCredentialV1,
        HouseProviderPortErrorV1, HouseProviderPortV1, HouseProviderReplyV1,
        HouseProviderRequestV1, HouseSpendLimitsV1, build_provider_request, number_to_nano_usd,
        parse_openrouter_http_response, provider_max_price,
    };

    const REVISION: &[u8] =
        include_bytes!("../../../config/hosted/house-agents/cooperative-planner-1.json");

    fn revision() -> Result<HouseAgentRevision, Box<dyn Error>> {
        let canonical = CanonicalJsonV1::parse(REVISION)?.to_bytes()?;
        Ok(HouseAgentRevision::from_canonical_bytes(&canonical)?)
    }

    fn period() -> Result<HouseAllowancePeriodV1, Box<dyn Error>> {
        Ok(HouseAllowancePeriodV1::from_unix_seconds(1_788_000_000)?)
    }

    fn identity(
        assignment: &str,
        activation: &str,
    ) -> Result<HouseInvocationIdentityV1, Box<dyn Error>> {
        Ok(HouseInvocationIdentityV1::new(
            assignment,
            activation,
            1,
            &format!("blake3:{}", "a".repeat(64)),
        )?)
    }

    fn credential() -> Result<HouseProviderCredentialV1, Box<dyn Error>> {
        Ok(HouseProviderCredentialV1::new(Zeroizing::new(
            b"sk-test-01234567890123456789012345678901".to_vec(),
        ))?)
    }

    /// No real credential or model execution: exercises production TLS against
    /// the fixed service and expects its authentication rejection, not EOF.
    #[test]
    #[ignore = "explicit network diagnostic; invalid fixture key, no paid model call"]
    fn production_tls_reads_authentication_rejection_without_half_close()
    -> Result<(), Box<dyn Error>> {
        let request = HouseProviderRequestV1 {
            body: Zeroizing::new(b"{}".to_vec()),
            timeout: std::time::Duration::from_secs(10),
            model_slug: "unused".to_owned(),
            provider_slug: "unused".to_owned(),
        };
        assert_eq!(
            super::OpenRouterProviderPortV1::new()
                .dispatch(&request, &credential()?)
                .err(),
            Some(HouseProviderPortErrorV1::Rejected)
        );
        Ok(())
    }

    fn offers() -> Value {
        json!({
            "offers": [{
                "action_type": "wait",
                "offer_id": "4:0:blake3-offer",
                "payload_schema": {
                    "schema": {
                        "additionalProperties": false,
                        "properties": {},
                        "type": "object"
                    },
                    "schema_digest": format!("blake3:{}", "b".repeat(64)),
                    "schema_id": "agent-heist/wait/v1"
                }
            }],
            "precondition": {
                "head_hash": format!("blake3:{}", "c".repeat(64)),
                "room_seq": 4
            },
            "schema": "worldstream/assignment-action-offer-list/v1"
        })
    }

    fn action(payload: Value) -> HouseProposedActionV1 {
        HouseProposedActionV1 {
            offer_id: "4:0:blake3-offer".to_owned(),
            payload,
        }
    }

    struct MetadataReplyProvider(Option<Value>);

    impl HouseProviderPortV1 for MetadataReplyProvider {
        fn dispatch(
            &self,
            request: &HouseProviderRequestV1,
            _credential: &HouseProviderCredentialV1,
        ) -> Result<HouseProviderReplyV1, HouseProviderPortErrorV1> {
            let mut response = json!({
                "id": "gen-documented-metadata",
                "object": "chat.completion",
                "model": request.model_slug(),
                "choices": [{
                    "index": 0,
                    "finish_reason": "stop",
                    "message": {
                        "role": "assistant",
                        "content": "{\"offer_id\":\"4:0:blake3-offer\",\"payload\":{}}"
                    }
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            });
            if let Some(metadata) = &self.0 {
                response["openrouter_metadata"] = metadata.clone();
            }
            let body =
                serde_json::to_vec(&response).map_err(|_| HouseProviderPortErrorV1::Rejected)?;
            HouseProviderReplyV1::new(body)
        }
    }

    fn first_attempt_metadata() -> Value {
        // OpenRouter's documented direct-success shape permits omitted `attempts`.
        json!({
            "requested": "qwen/qwen3.8-flash-20260826",
            "strategy": "direct",
            "region": "iad",
            "summary": "available=1, selected=Alibaba",
            "attempt": 1,
            "is_byok": false,
            "endpoints": {
                "total": 1,
                "available": [{
                    "provider": "Alibaba",
                    "model": "qwen/qwen3.8-flash-20260826",
                    "selected": true
                }]
            }
        })
    }

    #[test]
    fn first_attempt_metadata_without_optional_attempts_returns_action()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let executor = HouseModelExecutorV1::new(
            MetadataReplyProvider(Some(first_attempt_metadata())),
            ledger.clone(),
        );
        let completion = executor.execute(
            &credential()?,
            &revision()?,
            &identity("metadata-success", "metadata-success")?,
            &json!({"phase": "planning"}),
            &offers(),
            period()?,
        )?;
        assert_eq!(completion.action.offer_id, "4:0:blake3-offer");
        assert_eq!(completion.action.payload, json!({}));
        assert_eq!(completion.evidence.generation_id, "gen-documented-metadata");
        let usage = ledger.usage("metadata-success")?;
        assert_eq!(usage.attempts, 1);
        assert_eq!(usage.active_calls, 0);
        assert!(usage.consumed_output_units < 1_000);
        Ok(())
    }

    #[test]
    fn granite_response_uses_catalog_model_id_and_prefilter_endpoint_count()
    -> Result<(), Box<dyn Error>> {
        let canonical = CanonicalJsonV1::parse(include_bytes!(
            "../../../config/hosted/house-agents/cooperative-planner-4.json"
        ))?
        .to_bytes()?;
        let revision = HouseAgentRevision::from_canonical_bytes(&canonical)?;
        let (request, schemas) = build_provider_request(&revision, &json!({}), &offers())?;
        let mut metadata = first_attempt_metadata();
        metadata["requested"] = json!(revision.model_slug());
        metadata["endpoints"] = json!({
            "total": 2,
            "available": [{"provider":"DeepInfra", "model":revision.model_slug(), "selected":true}]
        });
        let reply = MetadataReplyProvider(Some(metadata)).dispatch(&request, &credential()?)?;
        let mut wire: Value = serde_json::from_slice(reply.body())?;
        wire["model"] = json!("ibm-granite/granite-4.2-8b");
        let decode = |value: &Value| {
            let reply = HouseProviderReplyV1::new(serde_json::to_vec(value).unwrap()).unwrap();
            super::decode_provider_reply(&reply, &revision, &schemas, "fixture-request")
        };
        assert!(decode(&wire).is_ok());
        for (pointer, invalid) in [
            ("/model", json!("ibm-granite/granite-4.1-8b")),
            ("/openrouter_metadata/requested", json!("another/model")),
            ("/openrouter_metadata/endpoints/total", json!(0)),
            (
                "/openrouter_metadata/endpoints/available/0/model",
                json!("another/model"),
            ),
            (
                "/openrouter_metadata/endpoints/available/0/provider",
                json!("CoreWeave"),
            ),
            ("/openrouter_metadata/attempt", json!(2)),
        ] {
            let mut changed = wire.clone();
            *changed
                .pointer_mut(pointer)
                .ok_or("invalid fixture pointer")? = invalid;
            assert!(decode(&changed).is_err(), "{pointer}");
        }
        Ok(())
    }

    #[test]
    fn null_attempt_history_is_invalid_and_consumes_the_paid_attempt() -> Result<(), Box<dyn Error>>
    {
        let directory = tempdir()?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let mut metadata = first_attempt_metadata();
        metadata["attempts"] = Value::Null;
        let executor =
            HouseModelExecutorV1::new(MetadataReplyProvider(Some(metadata)), ledger.clone());
        let result = executor.execute(
            &credential()?,
            &revision()?,
            &identity("metadata-null", "metadata-null")?,
            &json!({"phase": "planning"}),
            &offers(),
            period()?,
        );
        assert_eq!(result.err(), Some(HouseModelErrorV1::InvalidResponse));
        let usage = ledger.usage("metadata-null")?;
        assert_eq!(usage.attempts, 1);
        assert_eq!(usage.active_calls, 0);
        assert_eq!(usage.consumed_output_units, 1_000);
        Ok(())
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "the bounded response matrix keeps each rejection beside its public execution checks"
    )]
    fn present_attempt_history_and_required_route_evidence_remain_strict()
    -> Result<(), Box<dyn Error>> {
        let success = json!({
            "provider": "Alibaba", "model": "qwen/qwen3.8-flash-20260826", "status": 200
        });
        let cases = [
            ("valid-history", "/attempts", json!([success]), None),
            (
                "empty-history",
                "/attempts",
                json!([]),
                Some(HouseModelErrorV1::FallbackDetected),
            ),
            (
                "retry-history",
                "/attempts",
                json!([success, success]),
                Some(HouseModelErrorV1::FallbackDetected),
            ),
            (
                "invalid-history",
                "/attempts",
                json!({}),
                Some(HouseModelErrorV1::InvalidResponse),
            ),
            (
                "missing-status",
                "/attempts",
                json!([{"provider": "Alibaba", "model": "qwen/qwen3.8-flash-20260826"}]),
                Some(HouseModelErrorV1::InvalidResponse),
            ),
            (
                "history-model",
                "/attempts/0/model",
                json!("another/model"),
                Some(HouseModelErrorV1::RouteMismatch),
            ),
            (
                "history-provider",
                "/attempts/0/provider",
                json!("AnotherProvider"),
                Some(HouseModelErrorV1::RouteMismatch),
            ),
            (
                "history-status",
                "/attempts/0/status",
                json!(503),
                Some(HouseModelErrorV1::RouteMismatch),
            ),
            (
                "retry-counter",
                "/attempt",
                json!(2),
                Some(HouseModelErrorV1::FallbackDetected),
            ),
            (
                "missing-metadata",
                "",
                Value::Null,
                Some(HouseModelErrorV1::InvalidResponse),
            ),
            (
                "null-metadata",
                "",
                Value::Null,
                Some(HouseModelErrorV1::InvalidResponse),
            ),
            (
                "unselected",
                "/endpoints/available/0/selected",
                json!(false),
                Some(HouseModelErrorV1::RouteMismatch),
            ),
            (
                "endpoint-model",
                "/endpoints/available/0/model",
                json!("another/model"),
                Some(HouseModelErrorV1::RouteMismatch),
            ),
            (
                "endpoint-provider",
                "/endpoints/available/0/provider",
                json!("AnotherProvider"),
                Some(HouseModelErrorV1::RouteMismatch),
            ),
        ];
        for (name, pointer, value, expected) in cases {
            let directory = tempdir()?;
            let ledger = FileHouseAllowanceLedgerV1::open_at(
                directory.path().join("ledger"),
                HouseSpendLimitsV1::hobby_preview(),
                period()?,
            )?;
            let mut metadata = first_attempt_metadata();
            metadata["attempts"] = json!([success]);
            *metadata
                .pointer_mut(pointer)
                .ok_or("invalid fixture pointer")? = value;
            let metadata = (name != "missing-metadata").then_some(metadata);
            let executor =
                HouseModelExecutorV1::new(MetadataReplyProvider(metadata), ledger.clone());
            let result = executor.execute(
                &credential()?,
                &revision()?,
                &identity(name, name)?,
                &json!({"phase": "planning"}),
                &offers(),
                period()?,
            );
            if let Some(expected) = expected {
                assert_eq!(result.err(), Some(expected), "{name}");
                assert_eq!(ledger.usage(name)?.consumed_output_units, 1_000);
            } else {
                assert_eq!(result?.action.offer_id, "4:0:blake3-offer");
            }
            assert_eq!(ledger.usage(name)?.attempts, 1);
            assert_eq!(ledger.usage(name)?.active_calls, 0);
        }
        Ok(())
    }

    #[test]
    fn request_pins_every_route_privacy_and_output_control() -> Result<(), Box<dyn Error>> {
        let revision = revision()?;
        let (request, offered) =
            build_provider_request(&revision, &json!({"phase": "planning"}), &offers())?;
        let body: Value = serde_json::from_slice(request.body())?;
        let object = body.as_object().ok_or("request was not an object")?;
        let keys = object.keys().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(
            keys,
            vec![
                "max_tokens",
                "messages",
                "model",
                "provider",
                "response_format",
                "stream"
            ]
        );
        assert_eq!(
            body.get("model").and_then(Value::as_str),
            Some(revision.model_slug())
        );
        assert_eq!(body.get("max_tokens").and_then(Value::as_u64), Some(1_000));
        assert_eq!(body.get("stream").and_then(Value::as_bool), Some(false));
        assert!(body.get("tools").is_none());
        assert!(body.get("plugins").is_none());
        let provider = body
            .get("provider")
            .and_then(Value::as_object)
            .ok_or("provider controls missing")?;
        assert_eq!(
            provider.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![
                "allow_fallbacks",
                "data_collection",
                "max_price",
                "only",
                "order",
                "require_parameters",
                "zdr"
            ]
        );
        assert_eq!(provider.get("allow_fallbacks"), Some(&Value::Bool(false)));
        assert_eq!(provider.get("require_parameters"), Some(&Value::Bool(true)));
        assert_eq!(provider.get("zdr"), Some(&Value::Bool(true)));
        assert_eq!(
            provider.get("data_collection").and_then(Value::as_str),
            Some("deny")
        );
        assert_eq!(
            provider
                .get("only")
                .and_then(Value::as_array)
                .and_then(|values| values.first())
                .and_then(Value::as_str),
            Some(revision.provider_slug())
        );
        assert_eq!(
            provider
                .get("max_price")
                .and_then(|value| value.get("prompt"))
                .map(Value::to_string)
                .as_deref(),
            Some("0.15")
        );
        assert_eq!(request.timeout().as_secs(), 60);
        assert!(offered.contains_key("4:0:blake3-offer"));
        Ok(())
    }

    #[test]
    fn deterministic_success_returns_only_validated_action_and_safe_evidence()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let provider = Arc::new(DeterministicHouseProviderPortV1::new(
            action(json!({})),
            DeterministicHouseProviderFaultV1::None,
        ));
        let executor = HouseModelExecutorV1::new(Arc::clone(&provider), ledger.clone());
        let completion = executor.execute(
            &credential()?,
            &revision()?,
            &identity("assignment-success", "activation-success")?,
            &json!({"phase": "planning"}),
            &offers(),
            period()?,
        )?;
        assert_eq!(completion.action.offer_id, "4:0:blake3-offer");
        assert_eq!(completion.action.payload, json!({}));
        assert_eq!(
            completion.evidence.generation_id,
            "gen-worldstream-deterministic"
        );
        assert!(completion.evidence.request_digest.starts_with("blake3:"));
        assert!(completion.evidence.response_digest.starts_with("blake3:"));
        assert_eq!(provider.call_count(), 1);
        let usage = ledger.usage("assignment-success")?;
        assert_eq!(usage.attempts, 1);
        assert_eq!(usage.active_calls, 0);
        assert!(usage.consumed_input_units > 0);
        assert!(usage.consumed_output_units < 1_000);
        Ok(())
    }

    #[test]
    fn conformance_faults_fail_closed_and_consume_one_attempt() -> Result<(), Box<dyn Error>> {
        let cases = [
            (
                DeterministicHouseProviderFaultV1::InvalidSchema,
                HouseModelErrorV1::InvalidResponse,
            ),
            (
                DeterministicHouseProviderFaultV1::RouteMismatch,
                HouseModelErrorV1::RouteMismatch,
            ),
            (
                DeterministicHouseProviderFaultV1::FallbackEvidence,
                HouseModelErrorV1::FallbackDetected,
            ),
            (
                DeterministicHouseProviderFaultV1::OutputTooLarge,
                HouseModelErrorV1::OutputLimit,
            ),
            (
                DeterministicHouseProviderFaultV1::Timeout,
                HouseModelErrorV1::Provider(HouseProviderPortErrorV1::Timeout),
            ),
            (
                DeterministicHouseProviderFaultV1::LostReply,
                HouseModelErrorV1::Provider(HouseProviderPortErrorV1::LostReply),
            ),
            (
                DeterministicHouseProviderFaultV1::RateLimited,
                HouseModelErrorV1::Provider(HouseProviderPortErrorV1::RateLimited),
            ),
            (
                DeterministicHouseProviderFaultV1::Rejected,
                HouseModelErrorV1::Provider(HouseProviderPortErrorV1::Rejected),
            ),
        ];
        for (index, (fault, expected)) in cases.into_iter().enumerate() {
            let directory = tempdir()?;
            let ledger = FileHouseAllowanceLedgerV1::open_at(
                directory.path().join("ledger"),
                HouseSpendLimitsV1::hobby_preview(),
                period()?,
            )?;
            let provider = Arc::new(DeterministicHouseProviderPortV1::new(
                action(json!({})),
                fault,
            ));
            let executor = HouseModelExecutorV1::new(Arc::clone(&provider), ledger.clone());
            let assignment_id = format!("assignment-fault-{index}");
            let result = executor.execute(
                &credential()?,
                &revision()?,
                &identity(&assignment_id, &format!("activation-fault-{index}"))?,
                &json!({"phase": "planning"}),
                &offers(),
                period()?,
            );
            assert_eq!(result.err(), Some(expected));
            assert_eq!(provider.call_count(), 1);
            let usage = ledger.usage(&assignment_id)?;
            assert_eq!(usage.attempts, 1);
            assert_eq!(usage.active_calls, 0);
            assert_eq!(usage.consumed_output_units, 1_000);
        }
        Ok(())
    }

    #[test]
    fn invalid_action_payload_is_rejected_after_provider_dispatch() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let provider = Arc::new(DeterministicHouseProviderPortV1::new(
            action(json!({"not_allowed": true})),
            DeterministicHouseProviderFaultV1::None,
        ));
        let executor = HouseModelExecutorV1::new(Arc::clone(&provider), ledger.clone());
        let result = executor.execute(
            &credential()?,
            &revision()?,
            &identity("assignment-invalid-payload", "activation-invalid-payload")?,
            &json!({"phase": "planning"}),
            &offers(),
            period()?,
        );
        assert_eq!(result.err(), Some(HouseModelErrorV1::InvalidResponse));
        assert_eq!(provider.call_count(), 1);
        assert_eq!(
            ledger
                .usage("assignment-invalid-payload")?
                .consumed_output_units,
            1_000
        );
        Ok(())
    }

    #[test]
    fn oversized_input_fails_before_reservation_or_dispatch() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let provider = Arc::new(DeterministicHouseProviderPortV1::new(
            action(json!({})),
            DeterministicHouseProviderFaultV1::None,
        ));
        let executor = HouseModelExecutorV1::new(Arc::clone(&provider), ledger.clone());
        let result = executor.execute(
            &credential()?,
            &revision()?,
            &identity("assignment-oversized", "activation-oversized")?,
            &json!({"large": "x".repeat(20_000)}),
            &offers(),
            period()?,
        );
        assert_eq!(result.err(), Some(HouseModelErrorV1::InputLimit));
        assert_eq!(provider.call_count(), 0);
        assert_eq!(
            ledger.usage("assignment-oversized").err(),
            Some(HouseAllowanceErrorV1::InvalidInput)
        );
        Ok(())
    }

    #[test]
    fn lost_reply_is_never_redispatched_for_the_same_attempt() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let provider = Arc::new(DeterministicHouseProviderPortV1::new(
            action(json!({})),
            DeterministicHouseProviderFaultV1::LostReply,
        ));
        let executor = HouseModelExecutorV1::new(Arc::clone(&provider), ledger);
        let exact_identity = identity("assignment-lost", "activation-lost")?;
        let first = executor.execute(
            &credential()?,
            &revision()?,
            &exact_identity,
            &json!({"phase": "planning"}),
            &offers(),
            period()?,
        );
        assert_eq!(
            first.err(),
            Some(HouseModelErrorV1::Provider(
                HouseProviderPortErrorV1::LostReply
            ))
        );
        let second = executor.execute(
            &credential()?,
            &revision()?,
            &exact_identity,
            &json!({"phase": "planning"}),
            &offers(),
            period()?,
        );
        assert_eq!(
            second.err(),
            Some(HouseModelErrorV1::Allowance(
                HouseAllowanceErrorV1::AttemptConsumed
            ))
        );
        assert_eq!(provider.call_count(), 1);
        Ok(())
    }

    #[test]
    fn restart_converts_inflight_reservation_to_full_ambiguous_consumption()
    -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let exact_identity = identity("assignment-restart", "activation-restart")?;
        let revision = revision()?;
        {
            let ledger = FileHouseAllowanceLedgerV1::open_at(
                directory.path().join("ledger"),
                HouseSpendLimitsV1::hobby_preview(),
                period()?,
            )?;
            let _reservation = ledger.reserve(&exact_identity, &revision, 100, period()?)?;
        }
        let recovered = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let usage = recovered.usage("assignment-restart")?;
        assert_eq!(usage.active_calls, 0);
        assert_eq!(usage.consumed_input_units, 100);
        assert_eq!(usage.consumed_output_units, 1_000);
        assert_eq!(
            recovered
                .reserve(&exact_identity, &revision, 100, period()?)
                .err(),
            Some(HouseAllowanceErrorV1::AttemptConsumed)
        );
        Ok(())
    }

    #[test]
    fn scoped_restart_does_not_reconcile_another_units_inflight_call() -> Result<(), Box<dyn Error>>
    {
        let directory = tempdir()?;
        let root = directory.path().join("ledger");
        let first = identity("house-unit-first", "activation-first")?;
        let second = identity("house-unit-second", "activation-second")?;
        let revision = revision()?;
        {
            let ledger = FileHouseAllowanceLedgerV1::open_at(
                &root,
                HouseSpendLimitsV1::hobby_preview(),
                period()?,
            )?;
            let _first_reservation = ledger.reserve(&first, &revision, 100, period()?)?;
            let _second_reservation = ledger.reserve(&second, &revision, 100, period()?)?;
        }

        let recovered = FileHouseAllowanceLedgerV1::open_at_scope(
            &root,
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
            Some("house-unit-first"),
        )?;
        assert_eq!(recovered.usage("house-unit-first")?.active_calls, 0);
        assert_eq!(recovered.usage("house-unit-second")?.active_calls, 1);
        assert_eq!(
            recovered.reserve(&first, &revision, 100, period()?).err(),
            Some(HouseAllowanceErrorV1::AttemptConsumed)
        );
        assert_eq!(
            recovered
                .reserve(
                    &identity("house-unit-second", "activation-new")?,
                    &revision,
                    100,
                    period()?
                )
                .err(),
            Some(HouseAllowanceErrorV1::ConcurrentCall)
        );
        Ok(())
    }

    #[test]
    fn concurrency_call_and_spend_gates_are_atomic() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("concurrency"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let revision = revision()?;
        let first = ledger.reserve(
            &identity("assignment-gates", "activation-0")?,
            &revision,
            100,
            period()?,
        )?;
        assert_eq!(
            ledger
                .reserve(
                    &identity("assignment-gates", "activation-concurrent")?,
                    &revision,
                    100,
                    period()?
                )
                .err(),
            Some(HouseAllowanceErrorV1::ConcurrentCall)
        );
        ledger.consume_failed(&first, AttemptStateV1::ProviderFailed)?;
        for index in 1..10 {
            let reservation = ledger.reserve(
                &identity("assignment-gates", &format!("activation-{index}"))?,
                &revision,
                100,
                period()?,
            )?;
            ledger.consume_failed(&reservation, AttemptStateV1::ProviderFailed)?;
        }
        assert_eq!(
            ledger
                .reserve(
                    &identity("assignment-gates", "activation-11")?,
                    &revision,
                    100,
                    period()?
                )
                .err(),
            Some(HouseAllowanceErrorV1::CallLimit)
        );

        let spend_ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("spend"),
            HouseSpendLimitsV1 {
                daily_nano_usd: 1,
                monthly_nano_usd: 1,
            },
            period()?,
        )?;
        assert_eq!(
            spend_ledger
                .reserve(
                    &identity("assignment-spend", "activation-spend")?,
                    &revision,
                    100,
                    period()?
                )
                .err(),
            Some(HouseAllowanceErrorV1::SpendLimit)
        );
        Ok(())
    }

    #[test]
    fn ledger_never_persists_prompts_responses_or_credentials() -> Result<(), Box<dyn Error>> {
        let directory = tempdir()?;
        let ledger = FileHouseAllowanceLedgerV1::open_at(
            directory.path().join("ledger"),
            HouseSpendLimitsV1::hobby_preview(),
            period()?,
        )?;
        let provider = DeterministicHouseProviderPortV1::new(
            action(json!({})),
            DeterministicHouseProviderFaultV1::RateLimited,
        );
        let executor = HouseModelExecutorV1::new(provider, ledger);
        let secret = "sk-test-ledger-must-never-retain-1234567890";
        let private = HouseProviderCredentialV1::new(Zeroizing::new(secret.as_bytes().to_vec()))?;
        let marker = "PRIVATE_PROJECTION_MARKER";
        let _result = executor.execute(
            &private,
            &revision()?,
            &identity("assignment-private", "activation-private")?,
            &json!({"content": marker}),
            &offers(),
            period()?,
        );
        let retained = fs::read_to_string(directory.path().join("ledger/allowances.json"))?;
        assert!(!retained.contains(secret));
        assert!(!retained.contains(marker));
        assert!(!retained.contains("gen-worldstream"));
        Ok(())
    }

    #[test]
    fn fixed_http_decoder_rejects_redirects_and_accepts_bounded_chunking()
    -> Result<(), Box<dyn Error>> {
        let chunked = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n2\r\n{}\r\n0\r\n\r\n";
        let reply = parse_openrouter_http_response(chunked)?;
        assert_eq!(reply.body(), b"{}");
        let redirect = b"HTTP/1.1 302 Found\r\nContent-Type: application/json\r\nContent-Length: 2\r\nLocation: https://elsewhere.invalid/\r\n\r\n{}";
        assert_eq!(
            parse_openrouter_http_response(redirect).err(),
            Some(HouseProviderPortErrorV1::Rejected)
        );
        let rate_limit = b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(
            parse_openrouter_http_response(rate_limit).err(),
            Some(HouseProviderPortErrorV1::RateLimited)
        );
        Ok(())
    }

    #[test]
    fn development_provider_accepts_only_a_nonzero_loopback_address() {
        assert!(
            DevelopmentLoopbackOpenRouterProviderPortV1::new(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                8787
            )))
            .is_ok()
        );
        assert_eq!(
            DevelopmentLoopbackOpenRouterProviderPortV1::new(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                0,
            )))
            .err(),
            Some(HouseProviderPortErrorV1::Rejected)
        );
        assert_eq!(
            DevelopmentLoopbackOpenRouterProviderPortV1::new(std::net::SocketAddr::from((
                [192, 0, 2, 1],
                8787
            )))
            .err(),
            Some(HouseProviderPortErrorV1::Rejected)
        );
    }

    #[test]
    fn monetary_conversions_are_exact_and_conservative() -> Result<(), Box<dyn Error>> {
        assert_eq!(provider_max_price("0.00000015")?.to_string(), "0.15");
        assert_eq!(provider_max_price("0.000001")?.to_string(), "1");
        assert_eq!(number_to_nano_usd(&Number::from_str("0.0000012")?)?, 1_200);
        assert_eq!(number_to_nano_usd(&Number::from_str("1.2e-7")?)?, 120);
        assert_eq!(number_to_nano_usd(&Number::from_str("1.21e-10")?)?, 1);
        Ok(())
    }
}

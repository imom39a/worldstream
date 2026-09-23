//! Narrow persistence and `WorldStream` participant adapter seam.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::domain::{SwarmId, SwarmSummary, SwarmView, ValidatedCreateSwarm};

pub trait SwarmBackend {
    /// Creates exactly one new Swarm identity and its backing Room.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when validation, storage, or Room creation fails.
    fn create(&mut self, request: ValidatedCreateSwarm) -> Result<SwarmView, BackendError>;
    /// Lists the Swarms visible through this backend.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the durable index cannot be read safely.
    fn list(&self) -> Result<Vec<SwarmSummary>, BackendError>;
    /// Opens one exact Swarm by application identity.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::NotFound`] when absent, or another closed error
    /// when its authoritative projection cannot be read safely.
    fn open(&self, swarm_id: &SwarmId) -> Result<SwarmView, BackendError>;

    /// Observes one exact participant's current authorized context.
    ///
    /// The default keeps fixture-only backends honest: they do not silently
    /// impersonate a `WorldStream` participant or manufacture Action offers.
    ///
    /// # Errors
    ///
    /// Returns [`BackendError::Unsupported`] unless the adapter provides real
    /// participant authority, or another closed backend error on failure.
    fn observe(
        &self,
        _swarm_id: &SwarmId,
        _actor: &SwarmActor,
    ) -> Result<SwarmObservation, BackendError> {
        Err(BackendError::Unsupported)
    }

    /// Resolves the exact installed Pack payload schema for this offered Action.
    /// Implementations must verify the returned schema's canonical digest equals
    /// `offer.payload_schema_digest`; unavailable or mismatched schemas fail closed.
    ///
    /// # Errors
    /// Returns a closed error when the pinned schema cannot be verified.
    fn action_payload_schema(
        &self,
        _swarm_id: &SwarmId,
        _offer: &SwarmActionOffer,
    ) -> Result<Value, BackendError> {
        Err(BackendError::Unsupported)
    }

    /// Submits one exact currently offered Action for one participant.
    ///
    /// Callers supply both a stable Action identity and the Room sequence they
    /// evaluated. An adapter must never refresh those values implicitly. A
    /// changed Head returns [`BackendError::StaleObservation`], requiring the
    /// caller to re-observe and deliberately form a new Action.
    ///
    /// # Errors
    ///
    /// Returns a closed authority, freshness, storage, or uncertainty error.
    fn submit(
        &mut self,
        _swarm_id: &SwarmId,
        _request: &ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, BackendError> {
        Err(BackendError::Unsupported)
    }
}

/// Which retained Swarm participant is being observed or acting.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SwarmActor {
    HumanCoordinator,
    Worker { member_key: String },
}

/// One descriptor-backed Action offer from the participant's exact view.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwarmActionOffer {
    pub offer_id: String,
    pub action_type: String,
    pub payload_schema_digest: String,
}

/// Authorized current context. Artifact bytes and supervised process output do
/// not belong here; `activity` contains only the Pack's current projection.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SwarmObservation {
    pub swarm: SwarmView,
    pub actor: SwarmActor,
    /// Authenticated Membership identity for this exact actor. This comes
    /// from the scoped credential, not from a projection field.
    pub member_id: String,
    pub room_seq: u64,
    pub authoritative_state_hash: String,
    pub action_offers: Vec<SwarmActionOffer>,
    pub activity: Value,
}

/// Exact Action input retained by the caller before transport.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExactSwarmAction {
    pub actor: SwarmActor,
    pub action_id: String,
    pub based_on_room_seq: u64,
    pub offer_id: String,
    pub action_type: String,
    /// Exact schema identity retained with the offer. Keeping it here lets a
    /// lost reply be retried byte-for-byte without refreshing authority.
    pub payload_schema_digest: String,
    pub payload: Value,
}

/// Authoritative disposition of an exact Action identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum SwarmActionReceipt {
    Accepted {
        action_id: String,
        room_seq: u64,
        duplicate: bool,
    },
    Rejected {
        action_id: String,
        code: String,
        current_room_seq: u64,
        retryable_with_same_action_id: bool,
        may_submit_revised_action: bool,
        duplicate: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum BackendError {
    #[error("swarm was not found")]
    NotFound,
    #[error("backend data is invalid")]
    InvalidData,
    #[error("backend storage is unavailable")]
    StorageUnavailable,
    #[error("backend could not allocate a unique identity")]
    IdentityUnavailable,
    #[error("backend does not provide authoritative participant actions")]
    Unsupported,
    #[error("participant observation is stale; observe and re-evaluate")]
    StaleObservation,
    #[error("participant authority is unavailable or invalid")]
    AuthorityUnavailable,
    #[error("Action outcome is uncertain; reconcile the same Action identity")]
    ActionUncertain,
}

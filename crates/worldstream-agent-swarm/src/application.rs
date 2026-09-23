//! Create, list, and reopen application use cases.

use serde_json::Value;
use thiserror::Error;

use crate::{
    backend::{
        BackendError, ExactSwarmAction, SwarmActionOffer, SwarmActionReceipt, SwarmActor,
        SwarmBackend, SwarmObservation,
    },
    domain::{CreateSwarm, SwarmId, SwarmSummary, SwarmView, ValidationError},
};

pub struct SwarmApplication<B> {
    backend: B,
}

impl<B: SwarmBackend> SwarmApplication<B> {
    #[must_use]
    pub const fn new(backend: B) -> Self {
        Self { backend }
    }

    /// Validates a creation request and delegates one new Room to the backend.
    ///
    /// # Errors
    ///
    /// Returns validation failures before reaching the backend, or a closed
    /// backend error when creation cannot complete.
    pub fn create(&mut self, request: CreateSwarm) -> Result<SwarmView, ApplicationError> {
        self.backend
            .create(request.try_into().map_err(ApplicationError::Validation)?)
            .map_err(ApplicationError::Backend)
    }

    /// Lists Swarms known to the selected backend.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the index cannot be read safely.
    pub fn list(&self) -> Result<Vec<SwarmSummary>, ApplicationError> {
        self.backend.list().map_err(ApplicationError::Backend)
    }

    /// Opens an existing Swarm without creating a replacement Room.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when the Swarm is absent or unreadable.
    pub fn open(&self, swarm_id: &SwarmId) -> Result<SwarmView, ApplicationError> {
        self.backend
            .open(swarm_id)
            .map_err(ApplicationError::Backend)
    }

    /// Reads one participant's exact authorized Pack context.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error when participant authority, storage, or
    /// the current projection cannot be read safely.
    pub fn observe(
        &self,
        swarm_id: &SwarmId,
        actor: &SwarmActor,
    ) -> Result<SwarmObservation, ApplicationError> {
        self.backend
            .observe(swarm_id, actor)
            .map_err(ApplicationError::Backend)
    }

    /// Reads one exact offered Action's pinned payload schema.
    ///
    /// # Errors
    /// Returns a closed backend error when the schema cannot be verified.
    pub fn action_payload_schema(
        &self,
        swarm_id: &SwarmId,
        offer: &SwarmActionOffer,
    ) -> Result<Value, ApplicationError> {
        self.backend
            .action_payload_schema(swarm_id, offer)
            .map_err(ApplicationError::Backend)
    }

    /// Submits one exact caller-retained Action without changing its identity
    /// or silently refreshing its Room basis.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error for stale context, invalid authority,
    /// unavailable storage, or an uncertain transport outcome.
    pub fn submit(
        &mut self,
        swarm_id: &SwarmId,
        request: &ExactSwarmAction,
    ) -> Result<SwarmActionReceipt, ApplicationError> {
        self.backend
            .submit(swarm_id, request)
            .map_err(ApplicationError::Backend)
    }

    #[must_use]
    pub fn into_backend(self) -> B {
        self.backend
    }
}

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Validation(ValidationError),
    #[error(transparent)]
    Backend(BackendError),
}
